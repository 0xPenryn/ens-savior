mod bundle;
mod constants;
mod contracts;
mod ens;
mod rpc;
mod state;
mod types;
mod utils;

use std::str::FromStr;

use anyhow::{Context, Result, anyhow, bail};
use clap::Parser;
use reqwest::Client;

use crate::{
    bundle::{
        build_and_sign_bundle, bundle_included, estimate_required_funding, needs_eip7702_deauth,
        send_bundle, simulate_bundle, wait_for_funding,
    },
    constants::{BUILDER_NAMES, ENS_SUBGRAPH_FALLBACK_URL, ENS_SUBGRAPH_ID, FLASHBOTS_RPC},
    ens::{discover_names, plan_name_recoveries, select_names},
    state::{
        load_or_create_session, load_session, parse_compromised_signer, persist_completed,
        resolve_state_path,
    },
    types::{Cli, Commands, FlashbotsBundle, RecoverArgs, SweepArgs},
    utils::{gwei_to_wei, hex_to_bytes, parse_u128_hex, wei_to_eth_string},
};

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let http = Client::new();

    match cli.command {
        Commands::Recover(args) => recover_flow(&http, args).await,
        Commands::Sweep(args) => sweep_flow(&http, args).await,
    }
}

async fn recover_flow(http: &Client, args: RecoverArgs) -> Result<()> {
    let compromised_signer = parse_compromised_signer(&args)
        .context("failed to parse compromised wallet — provide --compromised-private-key or --compromised-mnemonic")?;
    let destination = alloy::primitives::Address::from_str(&args.destination)
        .with_context(|| format!("invalid destination address: {}", args.destination))?;
    let refund_address = args
        .refund_address
        .as_deref()
        .map(|addr| {
            alloy::primitives::Address::from_str(addr)
                .with_context(|| format!("invalid refund address: {addr}"))
        })
        .transpose()?;

    let subgraph_url = match (args.subgraph_url.as_deref(), args.subgraph_api_key.as_deref()) {
        (Some(url), _) => url.to_owned(),
        (None, Some(key)) => format!(
            "https://gateway.thegraph.com/api/{key}/subgraphs/id/{ENS_SUBGRAPH_ID}"
        ),
        (None, None) => {
            println!(
                "Warning: no --subgraph-api-key or --subgraph-url provided. \
                Falling back to the legacy hosted-service endpoint which is rate-limited and may be unreliable. \
                Get a free API key at https://thegraph.com/studio/apikeys/"
            );
            ENS_SUBGRAPH_FALLBACK_URL.to_owned()
        }
    };

    let state_path = resolve_state_path(&args, compromised_signer.address(), destination)?;
    let (session, funding_signer) =
        load_or_create_session(&state_path, compromised_signer.address(), destination)?;

    println!("Compromised wallet: {}", compromised_signer.address());
    println!("Destination wallet: {}", destination);
    println!("Funding wallet:     {}", funding_signer.address());
    println!("Session state:      {}", state_path.display());

    let chain_id = rpc::get_chain_id(http, &args.rpc_url).await?;

    let code = rpc::get_code(http, &args.rpc_url, compromised_signer.address()).await?;
    let needs_deauth = needs_eip7702_deauth(&code);
    if needs_deauth {
        println!(
            "Detected EIP-7702 delegated code on compromised wallet; deauth tx will be included."
        );
    } else {
        println!("Compromised wallet does not appear to need EIP-7702 deauth.");
    }

    let mut discovered = discover_names(
        http,
        &args.rpc_url,
        &subgraph_url,
        compromised_signer.address(),
    )
    .await?;

    if discovered.is_empty() {
        bail!("No ENS names discovered for the compromised wallet");
    }

    discovered.sort();
    let selected = select_names(&discovered)?;
    if selected.is_empty() {
        bail!("No ENS names selected");
    }

    let planned = plan_name_recoveries(
        http,
        &args.rpc_url,
        compromised_signer.address(),
        &selected,
    )
    .await?;

    println!("Planned recovery transactions:");
    for plan in &planned {
        println!("  - {} ({})", plan.name, plan.kind.as_str());
    }

    let latest_block = rpc::get_block_by_number(http, &args.rpc_url, "latest").await?;
    let base_fee = parse_u128_hex(
        latest_block
            .base_fee_per_gas
            .as_deref()
            .ok_or_else(|| anyhow!("latest block missing baseFeePerGas"))?,
    )?;
    let priority_fee = gwei_to_wei(args.priority_fee_gwei);
    let max_fee_per_gas = base_fee.saturating_mul(2).saturating_add(priority_fee);

    let (funding_needed, compromised_seed_value) = estimate_required_funding(
        &planned,
        needs_deauth,
        max_fee_per_gas,
        args.safety_buffer_pct,
    );

    println!(
        "Estimated required funding: {} wei ({} ETH)",
        funding_needed,
        wei_to_eth_string(funding_needed)
    );
    println!(
        "Compromised wallet seed transfer: {} wei ({} ETH)",
        compromised_seed_value,
        wei_to_eth_string(compromised_seed_value)
    );

    wait_for_funding(http, &args.rpc_url, funding_signer.address(), funding_needed).await?;

    let mut previous: Option<FlashbotsBundle> = None;
    let mut last_seen_block = 0u64;

    loop {
        let current_block = rpc::get_block_number(http, &args.rpc_url).await?;
        if current_block == last_seen_block {
            tokio::time::sleep(std::time::Duration::from_secs(2)).await;
            continue;
        }
        last_seen_block = current_block;
        println!("New block: {}", current_block);

        if let Some(ref bundle) = previous {
            if bundle_included(http, &args.rpc_url, &bundle.tx_hashes).await? {
                println!("Bundle included. Recovery complete.");
                persist_completed(&state_path, session)?;

                if let Some(refund) = refund_address {
                    rpc::sweep_funding_wallet(
                        http,
                        FLASHBOTS_RPC,
                        chain_id,
                        priority_fee,
                        &funding_signer,
                        refund,
                    )
                    .await?;
                } else {
                    println!(
                        "Warning: funding wallet {} may have leftover ETH. \
                        Run the following to recover it:\n  \
                        ens-savior sweep --state-path {} --refund-address <ADDRESS>",
                        funding_signer.address(),
                        state_path.display()
                    );
                }

                return Ok(());
            }
        }

        let block = rpc::get_block_by_number(http, &args.rpc_url, "latest").await?;
        let base_fee_now = parse_u128_hex(
            block
                .base_fee_per_gas
                .as_deref()
                .ok_or_else(|| anyhow!("latest block missing baseFeePerGas"))?,
        )?;
        let max_fee = base_fee_now.saturating_mul(2).saturating_add(priority_fee);
        let target_block = current_block + 1;

        let txs = build_and_sign_bundle(
            http,
            &args.rpc_url,
            chain_id,
            max_fee,
            priority_fee,
            needs_deauth,
            compromised_seed_value,
            &compromised_signer,
            &funding_signer,
            destination,
            &planned,
        )
        .await?;

        simulate_bundle(http, &args.relay_url, &funding_signer, &txs, target_block).await?;
        send_bundle(
            http,
            &args.relay_url,
            BUILDER_NAMES,
            &funding_signer,
            &txs,
            target_block,
        )
        .await?;

        let tx_hashes = txs
            .iter()
            .map(|tx| {
                let bytes = hex_to_bytes(tx)?;
                Ok(format!(
                    "0x{}",
                    alloy::primitives::hex::encode(alloy::primitives::keccak256(bytes))
                ))
            })
            .collect::<Result<Vec<_>>>()?;

        previous = Some(FlashbotsBundle { tx_hashes });
    }
}

async fn sweep_flow(http: &Client, args: SweepArgs) -> Result<()> {
    let session = load_session(&args.state_path)?;
    let funding_signer = state::parse_signer_from_key(&session.funding_private_key)?;
    let refund_address = alloy::primitives::Address::from_str(&args.refund_address)
        .with_context(|| format!("invalid refund address: {}", args.refund_address))?;

    println!("Funding wallet: {}", funding_signer.address());
    println!("Refund address: {}", refund_address);

    let chain_id = rpc::get_chain_id(http, FLASHBOTS_RPC).await?;
    let priority_fee = gwei_to_wei(1);

    rpc::sweep_funding_wallet(
        http,
        FLASHBOTS_RPC,
        chain_id,
        priority_fee,
        &funding_signer,
        refund_address,
    )
    .await
}

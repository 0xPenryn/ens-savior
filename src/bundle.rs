use std::time::Duration;

use alloy::{
    eips::eip7702::Authorization,
    network::EthereumWallet,
    primitives::{Address, U256, hex, keccak256},
    rpc::types::TransactionRequest,
    signers::{Signer, local::PrivateKeySigner},
};
use anyhow::{Context, Result, bail};
use dialoguer::{Confirm, theme::ColorfulTheme};
use reqwest::Client;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::{
    constants::{
        GAS_BASE_REG_TRANSFER, GAS_DEAUTH, GAS_FUND_TRANSFER, GAS_REGISTRY_SET_OWNER,
        GAS_WRAPPER_TRANSFER,
    },
    contracts, rpc,
    types::{PlannedNameTx, RecoveryKind},
    utils::{hex_to_bytes, wei_to_eth_string},
};

pub struct BundleBuildContext<'a> {
    pub http: &'a Client,
    pub rpc_url: &'a str,
    pub chain_id: u64,
    pub max_fee_per_gas: u128,
    pub max_priority_fee_per_gas: u128,
    pub compromised_signer: &'a PrivateKeySigner,
    pub funding_signer: &'a PrivateKeySigner,
}

pub struct BundlePlan<'a> {
    pub needs_deauth: bool,
    pub compromised_seed_value: U256,
    pub destination: Address,
    pub planned: &'a [PlannedNameTx],
}

pub fn needs_eip7702_deauth(code: &str) -> bool {
    let bytes = match hex_to_bytes(code) {
        Ok(v) => v,
        Err(_) => return false,
    };

    bytes.len() == 23 && bytes.starts_with(&[0xef, 0x01, 0x00])
}

pub fn estimate_required_funding(
    planned: &[PlannedNameTx],
    needs_deauth: bool,
    max_fee_per_gas: u128,
    safety_buffer_pct: u64,
) -> (U256, U256) {
    let compromised_gas: u64 = planned
        .iter()
        .map(|p| match p.kind {
            RecoveryKind::BaseRegistrar2ld { .. } => GAS_BASE_REG_TRANSFER,
            RecoveryKind::NameWrapper { .. } => GAS_WRAPPER_TRANSFER,
            RecoveryKind::RegistryOwner { .. } => GAS_REGISTRY_SET_OWNER,
        })
        .sum();

    let compromised_seed = U256::from(compromised_gas)
        .saturating_mul(U256::from(max_fee_per_gas))
        .saturating_mul(U256::from(100 + safety_buffer_pct))
        / U256::from(100u64);

    let funding_gas = GAS_FUND_TRANSFER + if needs_deauth { GAS_DEAUTH } else { 0 };
    let funding_gas_cost = U256::from(funding_gas)
        .saturating_mul(U256::from(max_fee_per_gas))
        .saturating_mul(U256::from(100 + safety_buffer_pct))
        / U256::from(100u64);

    (funding_gas_cost + compromised_seed, compromised_seed)
}

pub async fn wait_for_funding(
    http: &Client,
    rpc_url: &str,
    funding: Address,
    needed: U256,
) -> Result<()> {
    println!(
        "Send at least {} ETH to {}",
        wei_to_eth_string(needed),
        funding
    );

    let proceed = Confirm::with_theme(&ColorfulTheme::default())
        .with_prompt("Continue and wait for funding wallet balance?")
        .default(true)
        .interact()?;
    if !proceed {
        bail!("aborted by user");
    }

    loop {
        let balance = rpc::get_balance(http, rpc_url, funding).await?;
        if balance >= needed {
            println!("Funding wallet balance is sufficient.");
            return Ok(());
        }

        println!(
            "Funding wallet balance: {} ETH (need {})",
            wei_to_eth_string(balance),
            wei_to_eth_string(needed)
        );

        tokio::time::sleep(Duration::from_secs(5)).await;
    }
}

pub async fn build_and_sign_bundle(
    ctx: &BundleBuildContext<'_>,
    plan: &BundlePlan<'_>,
) -> Result<Vec<String>> {
    let compromised = ctx.compromised_signer.address();
    let funding = ctx.funding_signer.address();

    let funding_nonce = rpc::get_nonce(ctx.http, ctx.rpc_url, funding, "pending").await?;
    let compromised_nonce = rpc::get_nonce(ctx.http, ctx.rpc_url, compromised, "pending").await?;

    let funding_wallet = EthereumWallet::from(ctx.funding_signer.clone());
    let compromised_wallet = EthereumWallet::from(ctx.compromised_signer.clone());

    let mut txs = Vec::new();
    let mut funding_next_nonce = funding_nonce;
    let mut compromised_next_nonce = compromised_nonce;

    if plan.needs_deauth {
        let auth = Authorization {
            chain_id: U256::from(ctx.chain_id),
            address: Address::ZERO,
            nonce: compromised_nonce,
        };
        let sig = ctx.compromised_signer.sign_hash(&auth.signature_hash()).await?;
        let signed_auth = auth.into_signed(sig);

        let req = TransactionRequest::default()
            .from(funding)
            .to(compromised)
            .nonce(funding_next_nonce)
            .gas_limit(GAS_DEAUTH)
            .max_fee_per_gas(ctx.max_fee_per_gas)
            .max_priority_fee_per_gas(ctx.max_priority_fee_per_gas)
            .value(U256::ZERO);

        let mut req = req;
        req.chain_id = Some(ctx.chain_id);
        req.authorization_list = Some(vec![signed_auth]);

        let raw = rpc::sign_request_to_hex(&funding_wallet, req).await?;
        txs.push(raw);

        funding_next_nonce += 1;
        compromised_next_nonce += 1;
    }

    let seed_req = TransactionRequest::default()
        .from(funding)
        .to(compromised)
        .nonce(funding_next_nonce)
        .gas_limit(GAS_FUND_TRANSFER)
        .max_fee_per_gas(ctx.max_fee_per_gas)
        .max_priority_fee_per_gas(ctx.max_priority_fee_per_gas)
        .value(plan.compromised_seed_value);
    let mut seed_req = seed_req;
    seed_req.chain_id = Some(ctx.chain_id);
    txs.push(rpc::sign_request_to_hex(&funding_wallet, seed_req).await?);

    for item in plan.planned {
        let (to, data, gas) = match item.kind {
            RecoveryKind::BaseRegistrar2ld { token_id } => {
                let (to, data) =
                    contracts::build_base_registrar_transfer(compromised, plan.destination, token_id);
                (to, data, GAS_BASE_REG_TRANSFER)
            }
            RecoveryKind::NameWrapper { node } => {
                let (to, data) =
                    contracts::build_wrapper_transfer(compromised, plan.destination, node);
                (to, data, GAS_WRAPPER_TRANSFER)
            }
            RecoveryKind::RegistryOwner { node } => {
                let (to, data) = contracts::build_registry_set_owner(plan.destination, node);
                (to, data, GAS_REGISTRY_SET_OWNER)
            }
        };

        let req = TransactionRequest::default()
            .from(compromised)
            .to(to)
            .nonce(compromised_next_nonce)
            .gas_limit(gas)
            .max_fee_per_gas(ctx.max_fee_per_gas)
            .max_priority_fee_per_gas(ctx.max_priority_fee_per_gas)
            .value(U256::ZERO)
            .input(data.into());
        let mut req = req;
        req.chain_id = Some(ctx.chain_id);

        txs.push(rpc::sign_request_to_hex(&compromised_wallet, req).await?);
        compromised_next_nonce += 1;
    }

    Ok(txs)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SimBundle {
    total_gas_used: u64,
    bundle_gas_price: String,
    results: Vec<SimTx>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SimTx {
    tx_hash: String,
    gas_used: u64,
    #[serde(default)]
    error: Option<String>,
    #[serde(default)]
    revert: Option<String>,
}

pub async fn simulate_bundle(
    http: &Client,
    relay_url: &str,
    funding_signer: &PrivateKeySigner,
    txs: &[String],
    target_block: u64,
) -> Result<()> {
    println!("Bundle ({} txs):", txs.len());
    for (i, raw) in txs.iter().enumerate() {
        let bytes = hex_to_bytes(raw)?;
        let hash = keccak256(&bytes);
        println!("  [{i}] 0x{}", hex::encode(hash));
    }

    let body = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "eth_callBundle",
        "params": [
            {
                "txs": txs,
                "blockNumber": format!("0x{:x}", target_block),
                "stateBlockNumber": "latest"
            }
        ]
    })
    .to_string();

    let res = flashbots_request(http, relay_url, funding_signer, &body).await?;
    if res.get("error").is_some() {
        bail!("bundle simulation failed: {}", res);
    }

    if let Some(result) = res.get("result") {
        match serde_json::from_value::<SimBundle>(result.clone()) {
            Ok(sim) => {
                println!(
                    "Simulation OK — total gas: {}, gas price: {} wei",
                    sim.total_gas_used, sim.bundle_gas_price
                );
                for tx in &sim.results {
                    if let Some(err) = tx.error.as_deref().or(tx.revert.as_deref()) {
                        println!("  {} — gas: {} REVERTED: {}", tx.tx_hash, tx.gas_used, err);
                    } else {
                        println!("  {} — gas: {}", tx.tx_hash, tx.gas_used);
                    }
                }
            }
            Err(_) => println!("Simulation OK (could not parse result detail)"),
        }
    }

    Ok(())
}

pub async fn send_bundle(
    http: &Client,
    relay_url: &str,
    builder_names: &[&str],
    funding_signer: &PrivateKeySigner,
    txs: &[String],
    target_block: u64,
) -> Result<()> {
    let body = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "eth_sendBundle",
        "params": [
            {
                "txs": txs,
                "blockNumber": format!("0x{:x}", target_block),
                "builders": builder_names,
            }
        ]
    })
    .to_string();

    let res = flashbots_request(http, relay_url, funding_signer, &body).await?;
    if res.get("error").is_some() {
        bail!("bundle send failed: {}", res);
    }

    println!(
        "Bundle submitted for block {} via {} (multiplexing to {} builders)",
        target_block,
        relay_url,
        builder_names.len()
    );
    Ok(())
}

async fn flashbots_request(
    http: &Client,
    relay_url: &str,
    funding_signer: &PrivateKeySigner,
    body: &str,
) -> Result<Value> {
    // Flashbots requires signing the hex *string* of the body hash (not the raw bytes).
    // The relay verifies: ecrecover(keccak256("\x19Ethereum Signed Message:\n66" + hash_hex), sig)
    let body_hash = keccak256(body.as_bytes());
    let body_hash_hex = format!("0x{}", hex::encode(body_hash.as_slice()));
    let signature = funding_signer
        .sign_message(body_hash_hex.as_bytes())
        .await
        .context("failed to sign flashbots request")?;

    let header = format!("{}:{}", funding_signer.address(), signature);

    let res = http
        .post(relay_url)
        .header("Content-Type", "application/json")
        .header("X-Flashbots-Signature", header)
        .body(body.to_owned())
        .send()
        .await
        .with_context(|| format!("flashbots relay request failed: {}", relay_url))?
        .error_for_status()
        .context("flashbots relay returned non-2xx")?
        .json::<Value>()
        .await
        .context("failed to decode flashbots relay response")?;

    Ok(res)
}

pub async fn bundle_included(http: &Client, rpc_url: &str, tx_hashes: &[String]) -> Result<bool> {
    for tx_hash in tx_hashes {
        let result =
            rpc::rpc_call::<Value>(http, rpc_url, "eth_getTransactionReceipt", json!([tx_hash]))
                .await?;

        if !result.is_null() {
            return Ok(true);
        }
    }

    Ok(false)
}

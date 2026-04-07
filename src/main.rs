use std::{collections::BTreeSet, fs, path::PathBuf, str::FromStr, time::Duration};

use alloy::{
    eips::{eip2718::Encodable2718, eip7702::Authorization},
    ens::namehash,
    network::{Ethereum, EthereumWallet, NetworkWallet},
    primitives::{Address, B256, Bytes, U256, address, hex, keccak256},
    rpc::types::TransactionRequest,
    signers::{Signer, local::PrivateKeySigner},
    sol,
    sol_types::SolCall,
};
use anyhow::{Context, Result, anyhow, bail};
use clap::Parser;
use dialoguer::{Confirm, MultiSelect, theme::ColorfulTheme};
use reqwest::Client;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::{Value, json};

const ENS_BASE_REGISTRAR: Address = address!("57f1887a8BF19b14fC0dF6Fd9B2acc9Af147eA85");
const ENS_NAME_WRAPPER: Address = address!("D4416b13d2b3a9aBae7AcD5D6C2BbDBE25686401");
const ENS_REGISTRY: Address = address!("00000000000C2E074eC69A0dFb2997BA6C7d2e1e");

const DEFAULT_RPC: &str = "https://rpc.flashbots.net/fast";
const DEFAULT_RELAY: &str = "https://relay.flashbots.net";
const DEFAULT_SUBGRAPH: &str = "https://api.thegraph.com/subgraphs/name/ensdomains/ens";

const GAS_DEAUTH: u64 = 80_000;
const GAS_FUND_TRANSFER: u64 = 21_000;
const GAS_BASE_REG_TRANSFER: u64 = 120_000;
const GAS_WRAPPER_TRANSFER: u64 = 180_000;
const GAS_REGISTRY_SET_OWNER: u64 = 80_000;

sol! {
    interface IBaseRegistrar {
        function ownerOf(uint256 id) external view returns (address);
        function transferFrom(address from, address to, uint256 tokenId) external;
    }

    interface INameWrapper {
        function ownerOf(uint256 id) external view returns (address);
        function safeTransferFrom(address from, address to, uint256 id, uint256 amount, bytes data) external;
    }

    interface IENSRegistry {
        function owner(bytes32 node) external view returns (address);
        function setOwner(bytes32 node, address owner) external;
    }
}

#[derive(Debug, Parser)]
#[command(name = "ens-savior")]
#[command(about = "Recover ENS names from a compromised wallet via private bundle submission")]
struct Args {
    #[arg(long)]
    compromised_private_key: String,
    #[arg(long)]
    destination: String,
    #[arg(long, default_value = DEFAULT_RPC)]
    rpc_url: String,
    #[arg(long, default_value = DEFAULT_RELAY)]
    relay_url: String,
    #[arg(long, default_value = DEFAULT_SUBGRAPH)]
    subgraph_url: String,
    #[arg(long)]
    state_path: Option<PathBuf>,
    #[arg(long, default_value_t = 3)]
    priority_fee_gwei: u64,
    #[arg(long, default_value_t = 15)]
    safety_buffer_pct: u64,
}

#[derive(Debug, Serialize, Deserialize)]
struct SessionState {
    compromised: Address,
    destination: Address,
    funding_private_key: String,
    completed: bool,
}

#[derive(Debug, Clone)]
enum RecoveryKind {
    BaseRegistrar2ld { token_id: U256 },
    NameWrapper { node: U256 },
    RegistryOwner { node: B256 },
}

#[derive(Debug, Clone)]
struct PlannedNameTx {
    name: String,
    kind: RecoveryKind,
}

#[derive(Debug, Clone)]
struct FlashbotsBundle {
    tx_hashes: Vec<String>,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();
    let http = Client::new();

    let compromised_signer = parse_signer(&args.compromised_private_key)
        .context("failed to parse compromised private key")?;
    let destination = Address::from_str(&args.destination)
        .with_context(|| format!("invalid destination address: {}", args.destination))?;

    let state_path = resolve_state_path(&args, compromised_signer.address(), destination)?;
    let (session, funding_signer) =
        load_or_create_session(&state_path, compromised_signer.address(), destination)?;

    println!("Compromised wallet: {}", compromised_signer.address());
    println!("Destination wallet: {}", destination);
    println!("Funding wallet:     {}", funding_signer.address());
    println!("Session state:      {}", state_path.display());

    let chain_id = get_chain_id(&http, &args.rpc_url).await?;

    let code = get_code(&http, &args.rpc_url, compromised_signer.address()).await?;
    let needs_deauth = needs_eip7702_deauth(&code);
    if needs_deauth {
        println!(
            "Detected EIP-7702 delegated code on compromised wallet; deauth tx will be included."
        );
    } else {
        println!("Compromised wallet does not appear to need EIP-7702 deauth.");
    }

    let mut discovered = discover_names(
        &http,
        &args.rpc_url,
        &args.subgraph_url,
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
        &http,
        &args.rpc_url,
        compromised_signer.address(),
        &selected,
    )
    .await?;

    if planned.is_empty() {
        bail!("Could not plan any recoverable transactions for selected names");
    }

    println!("Planned recovery transactions:");
    for plan in &planned {
        println!("  - {} ({})", plan.name, plan.kind.as_str());
    }

    let latest_block = get_block_by_number(&http, &args.rpc_url, "latest").await?;
    let base_fee = latest_block
        .base_fee_per_gas
        .ok_or_else(|| anyhow!("latest block missing baseFeePerGas"))?;
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

    wait_for_funding(
        &http,
        &args.rpc_url,
        funding_signer.address(),
        funding_needed,
    )
    .await?;

    let mut previous: Option<FlashbotsBundle> = None;
    let mut last_seen_block = 0u64;

    loop {
        let current_block = get_block_number(&http, &args.rpc_url).await?;
        if current_block == last_seen_block {
            tokio::time::sleep(Duration::from_secs(2)).await;
            continue;
        }
        last_seen_block = current_block;
        println!("New block: {}", current_block);

        if let Some(ref bundle) = previous {
            if bundle_included(&http, &args.rpc_url, &bundle.tx_hashes).await? {
                println!("Bundle included. Recovery complete.");
                persist_completed(&state_path, session)?;
                return Ok(());
            }
        }

        let block = get_block_by_number(&http, &args.rpc_url, "latest").await?;
        let base_fee_now = block
            .base_fee_per_gas
            .ok_or_else(|| anyhow!("latest block missing baseFeePerGas"))?;
        let max_fee = base_fee_now.saturating_mul(2).saturating_add(priority_fee);
        let target_block = current_block + 1;

        let txs = build_and_sign_bundle(
            &http,
            &args.rpc_url,
            chain_id,
            target_block,
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

        simulate_bundle(&http, &args.relay_url, &funding_signer, &txs, target_block).await?;

        send_bundle(&http, &args.relay_url, &funding_signer, &txs, target_block).await?;

        let tx_hashes = txs
            .iter()
            .map(|tx| {
                let bytes = hex_to_bytes(tx)?;
                Ok(format!("0x{}", hex::encode(keccak256(bytes))))
            })
            .collect::<Result<Vec<_>>>()?;

        previous = Some(FlashbotsBundle { tx_hashes });
    }
}

impl RecoveryKind {
    fn as_str(&self) -> &'static str {
        match self {
            Self::BaseRegistrar2ld { .. } => "2LD .eth (BaseRegistrar ERC-721)",
            Self::NameWrapper { .. } => "Wrapped name (NameWrapper ERC-1155)",
            Self::RegistryOwner { .. } => "Registry owner-controlled name",
        }
    }
}

fn parse_signer(input: &str) -> Result<PrivateKeySigner> {
    let normalized = input.strip_prefix("0x").unwrap_or(input);
    let with_prefix = format!("0x{}", normalized);
    PrivateKeySigner::from_str(&with_prefix).map_err(Into::into)
}

fn resolve_state_path(args: &Args, compromised: Address, destination: Address) -> Result<PathBuf> {
    if let Some(path) = &args.state_path {
        return Ok(path.clone());
    }

    let mut base = dirs::config_dir().ok_or_else(|| anyhow!("unable to resolve config dir"))?;
    base.push("ens-savior");
    fs::create_dir_all(&base)?;
    base.push(format!("{}_{}.toml", compromised, destination));
    Ok(base)
}

fn load_or_create_session(
    state_path: &PathBuf,
    compromised: Address,
    destination: Address,
) -> Result<(SessionState, PrivateKeySigner)> {
    if state_path.exists() {
        let content = fs::read_to_string(state_path)?;
        let session: SessionState = toml::from_str(&content)?;
        if session.compromised != compromised || session.destination != destination {
            bail!(
                "session at {} does not match compromised/destination pair",
                state_path.display()
            );
        }

        let signer = parse_signer(&session.funding_private_key)?;
        return Ok((session, signer));
    }

    let signer = PrivateKeySigner::random();
    let session = SessionState {
        compromised,
        destination,
        funding_private_key: format!("0x{}", hex::encode(signer.to_bytes())),
        completed: false,
    };
    fs::write(state_path, toml::to_string_pretty(&session)?)?;
    Ok((session, signer))
}

fn persist_completed(state_path: &PathBuf, mut session: SessionState) -> Result<()> {
    session.completed = true;
    fs::write(state_path, toml::to_string_pretty(&session)?)?;
    Ok(())
}

fn needs_eip7702_deauth(code: &str) -> bool {
    let bytes = match hex_to_bytes(code) {
        Ok(v) => v,
        Err(_) => return false,
    };

    bytes.len() == 23 && bytes.starts_with(&[0xef, 0x01, 0x00])
}

fn label_to_token_id(label: &str) -> U256 {
    let hash = keccak256(label.as_bytes());
    U256::from_be_slice(hash.as_slice())
}

fn node_to_u256(node: [u8; 32]) -> U256 {
    U256::from_be_slice(&node)
}

async fn discover_names(
    http: &Client,
    rpc_url: &str,
    subgraph_url: &str,
    owner: Address,
) -> Result<Vec<String>> {
    let mut names = BTreeSet::new();

    for name in discover_names_subgraph(http, subgraph_url, owner).await? {
        names.insert(name);
    }

    if names.is_empty() {
        println!(
            "Subgraph did not return names; attempting best-effort on-chain reverse lookup is skipped."
        );
    }

    let mut filtered = Vec::new();
    for name in names {
        if name.is_empty() {
            continue;
        }

        let node = namehash(&name);
        let owner_on_registry = registry_owner(http, rpc_url, node).await?;
        let wrapped_owner = name_wrapper_owner(http, rpc_url, node_to_u256(node.0))
            .await
            .ok();

        if owner_on_registry == owner || wrapped_owner == Some(owner) || name.ends_with(".eth") {
            filtered.push(name);
        }
    }

    Ok(filtered)
}

#[derive(Debug, Deserialize)]
struct SubgraphDomainsResp {
    data: Option<SubgraphData>,
}

#[derive(Debug, Deserialize)]
struct SubgraphData {
    #[serde(default)]
    domains: Vec<SubgraphDomain>,
    #[serde(default)]
    wrapped_domains: Vec<WrappedDomain>,
}

#[derive(Debug, Deserialize)]
struct SubgraphDomain {
    name: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct WrappedDomain {
    name: Option<String>,
    domain: Option<SubgraphDomain>,
}

async fn discover_names_subgraph(
    http: &Client,
    subgraph_url: &str,
    owner: Address,
) -> Result<Vec<String>> {
    let owner_hex = format!("{owner:?}").to_lowercase();

    let query = r#"
        query($owner: String!) {
          domains(first: 1000, where: { owner: $owner }) {
            name
          }
          wrappedDomains(first: 1000, where: { owner: $owner }) {
            name
            domain { name }
          }
        }
    "#;

    let body = json!({
        "query": query,
        "variables": {
            "owner": owner_hex
        }
    });

    let resp = http
        .post(subgraph_url)
        .json(&body)
        .send()
        .await
        .with_context(|| format!("subgraph request failed: {}", subgraph_url))?
        .error_for_status()
        .with_context(|| format!("subgraph returned error status: {}", subgraph_url))?
        .json::<SubgraphDomainsResp>()
        .await
        .context("failed to decode subgraph response")?;

    let mut out = BTreeSet::new();
    if let Some(data) = resp.data {
        for domain in data.domains {
            if let Some(name) = domain.name {
                out.insert(name);
            }
        }

        for wrapped in data.wrapped_domains {
            if let Some(name) = wrapped.name {
                out.insert(name);
            }
            if let Some(domain) = wrapped.domain {
                if let Some(name) = domain.name {
                    out.insert(name);
                }
            }
        }
    }

    Ok(out.into_iter().collect())
}

fn select_names(names: &[String]) -> Result<Vec<String>> {
    let selection = MultiSelect::with_theme(&ColorfulTheme::default())
        .with_prompt("Select ENS names to recover")
        .items(names)
        .interact()?;

    Ok(selection
        .into_iter()
        .map(|idx| names[idx].clone())
        .collect())
}

async fn plan_name_recoveries(
    http: &Client,
    rpc_url: &str,
    compromised: Address,
    names: &[String],
) -> Result<Vec<PlannedNameTx>> {
    let mut planned = Vec::new();

    for name in names {
        if name.ends_with(".eth") && name.split('.').count() == 2 {
            let label = &name[..name.len() - 4];
            let token_id = label_to_token_id(label);
            if let Ok(owner) = base_registrar_owner(http, rpc_url, token_id).await {
                if owner == compromised {
                    planned.push(PlannedNameTx {
                        name: name.clone(),
                        kind: RecoveryKind::BaseRegistrar2ld { token_id },
                    });
                    continue;
                }
            }
        }

        let node = namehash(name);
        let node_u256 = node_to_u256(node.0);
        if let Ok(owner) = name_wrapper_owner(http, rpc_url, node_u256).await {
            if owner == compromised {
                planned.push(PlannedNameTx {
                    name: name.clone(),
                    kind: RecoveryKind::NameWrapper { node: node_u256 },
                });
                continue;
            }
        }

        let registry_owner_addr = registry_owner(http, rpc_url, node).await?;
        if registry_owner_addr == compromised {
            planned.push(PlannedNameTx {
                name: name.clone(),
                kind: RecoveryKind::RegistryOwner { node },
            });
            continue;
        }

        println!(
            "Skipping {}: owner is not compromised wallet in known ENS ownership paths",
            name
        );
    }

    Ok(planned)
}

fn estimate_required_funding(
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

async fn wait_for_funding(
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
        let balance = get_balance(http, rpc_url, funding).await?;
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

async fn build_and_sign_bundle(
    http: &Client,
    rpc_url: &str,
    chain_id: u64,
    _target_block: u64,
    max_fee_per_gas: u128,
    max_priority_fee_per_gas: u128,
    needs_deauth: bool,
    compromised_seed_value: U256,
    compromised_signer: &PrivateKeySigner,
    funding_signer: &PrivateKeySigner,
    destination: Address,
    planned: &[PlannedNameTx],
) -> Result<Vec<String>> {
    let compromised = compromised_signer.address();
    let funding = funding_signer.address();

    let funding_nonce = get_nonce(http, rpc_url, funding, "pending").await?;
    let compromised_nonce = get_nonce(http, rpc_url, compromised, "pending").await?;

    let funding_wallet = EthereumWallet::from(funding_signer.clone());
    let compromised_wallet = EthereumWallet::from(compromised_signer.clone());

    let mut txs = Vec::new();
    let mut funding_next_nonce = funding_nonce;
    let mut compromised_next_nonce = compromised_nonce;

    if needs_deauth {
        let auth = Authorization {
            chain_id: U256::from(chain_id),
            address: Address::ZERO,
            nonce: compromised_nonce,
        };
        let sig = compromised_signer.sign_hash(&auth.signature_hash()).await?;
        let signed_auth = auth.into_signed(sig);

        let req = TransactionRequest::default()
            .from(funding)
            .to(compromised)
            .nonce(funding_next_nonce)
            .gas_limit(GAS_DEAUTH)
            .max_fee_per_gas(max_fee_per_gas)
            .max_priority_fee_per_gas(max_priority_fee_per_gas)
            .value(U256::ZERO);

        let mut req = req;
        req.chain_id = Some(chain_id);
        req.authorization_list = Some(vec![signed_auth]);

        let raw = sign_request_to_hex(&funding_wallet, req).await?;
        txs.push(raw);

        funding_next_nonce += 1;
        compromised_next_nonce += 1;
    }

    let seed_req = TransactionRequest::default()
        .from(funding)
        .to(compromised)
        .nonce(funding_next_nonce)
        .gas_limit(GAS_FUND_TRANSFER)
        .max_fee_per_gas(max_fee_per_gas)
        .max_priority_fee_per_gas(max_priority_fee_per_gas)
        .value(compromised_seed_value);
    let mut seed_req = seed_req;
    seed_req.chain_id = Some(chain_id);
    txs.push(sign_request_to_hex(&funding_wallet, seed_req).await?);

    for item in planned {
        let (to, data, gas) = match item.kind {
            RecoveryKind::BaseRegistrar2ld { token_id } => {
                let call = IBaseRegistrar::transferFromCall {
                    from: compromised,
                    to: destination,
                    tokenId: token_id,
                };
                (
                    ENS_BASE_REGISTRAR,
                    Bytes::from(call.abi_encode()),
                    GAS_BASE_REG_TRANSFER,
                )
            }
            RecoveryKind::NameWrapper { node } => {
                let call = INameWrapper::safeTransferFromCall {
                    from: compromised,
                    to: destination,
                    id: node,
                    amount: U256::from(1),
                    data: Bytes::new(),
                };
                (
                    ENS_NAME_WRAPPER,
                    Bytes::from(call.abi_encode()),
                    GAS_WRAPPER_TRANSFER,
                )
            }
            RecoveryKind::RegistryOwner { node } => {
                let call = IENSRegistry::setOwnerCall {
                    node,
                    owner: destination,
                };
                (
                    ENS_REGISTRY,
                    Bytes::from(call.abi_encode()),
                    GAS_REGISTRY_SET_OWNER,
                )
            }
        };

        let req = TransactionRequest::default()
            .from(compromised)
            .to(to)
            .nonce(compromised_next_nonce)
            .gas_limit(gas)
            .max_fee_per_gas(max_fee_per_gas)
            .max_priority_fee_per_gas(max_priority_fee_per_gas)
            .value(U256::ZERO)
            .input(data.into());
        let mut req = req;
        req.chain_id = Some(chain_id);
        txs.push(sign_request_to_hex(&compromised_wallet, req).await?);
        compromised_next_nonce += 1;
    }

    Ok(txs)
}

async fn sign_request_to_hex(wallet: &EthereumWallet, req: TransactionRequest) -> Result<String> {
    let envelope = NetworkWallet::<Ethereum>::sign_request(wallet, req)
        .await
        .map_err(|e| anyhow!("failed to sign tx request: {e}"))?;

    Ok(format!("0x{}", hex::encode(envelope.encoded_2718())))
}

async fn simulate_bundle(
    http: &Client,
    relay_url: &str,
    funding_signer: &PrivateKeySigner,
    txs: &[String],
    target_block: u64,
) -> Result<()> {
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

    Ok(())
}

async fn send_bundle(
    http: &Client,
    relay_url: &str,
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
            }
        ]
    })
    .to_string();

    let res = flashbots_request(http, relay_url, funding_signer, &body).await?;
    if res.get("error").is_some() {
        bail!("bundle send failed: {}", res);
    }

    println!("Bundle submitted for block {}", target_block);
    Ok(())
}

async fn flashbots_request(
    http: &Client,
    relay_url: &str,
    funding_signer: &PrivateKeySigner,
    body: &str,
) -> Result<Value> {
    let body_hash = keccak256(body.as_bytes());
    let signature = funding_signer
        .sign_message(body_hash.as_slice())
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

async fn bundle_included(http: &Client, rpc_url: &str, tx_hashes: &[String]) -> Result<bool> {
    for tx_hash in tx_hashes {
        let result =
            rpc_call::<Value>(http, rpc_url, "eth_getTransactionReceipt", json!([tx_hash])).await?;

        if !result.is_null() {
            return Ok(true);
        }
    }

    Ok(false)
}

async fn base_registrar_owner(http: &Client, rpc_url: &str, token_id: U256) -> Result<Address> {
    let call = IBaseRegistrar::ownerOfCall { id: token_id };
    let output = eth_call(
        http,
        rpc_url,
        ENS_BASE_REGISTRAR,
        Bytes::from(call.abi_encode()),
    )
    .await?;
    let decoded = IBaseRegistrar::ownerOfCall::abi_decode_returns(&output)
        .map_err(|e| anyhow!("failed to decode base registrar ownerOf: {e}"))?;
    Ok(decoded)
}

async fn name_wrapper_owner(http: &Client, rpc_url: &str, node: U256) -> Result<Address> {
    let call = INameWrapper::ownerOfCall { id: node };
    let output = eth_call(
        http,
        rpc_url,
        ENS_NAME_WRAPPER,
        Bytes::from(call.abi_encode()),
    )
    .await?;
    let decoded = INameWrapper::ownerOfCall::abi_decode_returns(&output)
        .map_err(|e| anyhow!("failed to decode name wrapper ownerOf: {e}"))?;
    Ok(decoded)
}

async fn registry_owner(http: &Client, rpc_url: &str, node: B256) -> Result<Address> {
    let call = IENSRegistry::ownerCall { node };
    let output = eth_call(http, rpc_url, ENS_REGISTRY, Bytes::from(call.abi_encode())).await?;
    let decoded = IENSRegistry::ownerCall::abi_decode_returns(&output)
        .map_err(|e| anyhow!("failed to decode ens registry owner: {e}"))?;
    Ok(decoded)
}

async fn eth_call(http: &Client, rpc_url: &str, to: Address, data: Bytes) -> Result<Vec<u8>> {
    let call_obj = json!({
        "to": to,
        "data": format!("0x{}", hex::encode(data.as_ref())),
    });

    let out_hex: String = rpc_call(http, rpc_url, "eth_call", json!([call_obj, "latest"]))
        .await
        .context("eth_call failed")?;

    hex_to_bytes(&out_hex)
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RpcBlock {
    base_fee_per_gas: Option<u128>,
}

async fn get_block_by_number(http: &Client, rpc_url: &str, block: &str) -> Result<RpcBlock> {
    rpc_call(http, rpc_url, "eth_getBlockByNumber", json!([block, false])).await
}

async fn get_block_number(http: &Client, rpc_url: &str) -> Result<u64> {
    let hex_num: String = rpc_call(http, rpc_url, "eth_blockNumber", json!([])).await?;
    parse_u64_hex(&hex_num)
}

async fn get_chain_id(http: &Client, rpc_url: &str) -> Result<u64> {
    let hex_num: String = rpc_call(http, rpc_url, "eth_chainId", json!([])).await?;
    parse_u64_hex(&hex_num)
}

async fn get_nonce(http: &Client, rpc_url: &str, addr: Address, block_tag: &str) -> Result<u64> {
    let hex_num: String = rpc_call(
        http,
        rpc_url,
        "eth_getTransactionCount",
        json!([addr, block_tag]),
    )
    .await?;
    parse_u64_hex(&hex_num)
}

async fn get_balance(http: &Client, rpc_url: &str, addr: Address) -> Result<U256> {
    let hex_num: String =
        rpc_call(http, rpc_url, "eth_getBalance", json!([addr, "latest"])).await?;
    parse_u256_hex(&hex_num)
}

async fn get_code(http: &Client, rpc_url: &str, addr: Address) -> Result<String> {
    rpc_call(http, rpc_url, "eth_getCode", json!([addr, "latest"]))
        .await
        .context("eth_getCode failed")
}

async fn rpc_call<T: DeserializeOwned>(
    http: &Client,
    rpc_url: &str,
    method: &str,
    params: Value,
) -> Result<T> {
    let payload = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": method,
        "params": params,
    });

    let res = http
        .post(rpc_url)
        .json(&payload)
        .send()
        .await
        .with_context(|| format!("rpc request failed ({method})"))?
        .error_for_status()
        .with_context(|| format!("rpc non-2xx response ({method})"))?
        .json::<Value>()
        .await
        .context("failed to decode rpc response")?;

    if let Some(error) = res.get("error") {
        bail!("rpc error ({method}): {}", error);
    }

    let result = res
        .get("result")
        .ok_or_else(|| anyhow!("rpc response missing result field ({method})"))?
        .clone();

    serde_json::from_value(result)
        .with_context(|| format!("failed to parse rpc result for method {method}"))
}

fn parse_u64_hex(v: &str) -> Result<u64> {
    let stripped = v.strip_prefix("0x").unwrap_or(v);
    u64::from_str_radix(stripped, 16).map_err(Into::into)
}

fn parse_u256_hex(v: &str) -> Result<U256> {
    let stripped = v.strip_prefix("0x").unwrap_or(v);
    U256::from_str_radix(stripped, 16).map_err(Into::into)
}

fn gwei_to_wei(gwei: u64) -> u128 {
    (gwei as u128) * 1_000_000_000u128
}

fn wei_to_eth_string(wei: U256) -> String {
    let scale = U256::from(1_000_000_000_000_000_000u128);
    let whole = wei / scale;
    let frac = (wei % scale) / U256::from(1_000_000_000_000u64);
    format!("{}.{:06}", whole, frac)
}

fn hex_to_bytes(s: &str) -> Result<Vec<u8>> {
    let stripped = s.strip_prefix("0x").unwrap_or(s);
    if stripped.is_empty() {
        return Ok(Vec::new());
    }
    Ok(hex::decode(stripped)?)
}

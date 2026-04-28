use std::time::Duration;

use alloy::{
    eips::eip2718::Encodable2718,
    network::{Ethereum, EthereumWallet, NetworkWallet},
    primitives::{Address, Bytes, U256, hex},
    rpc::types::TransactionRequest,
    signers::local::PrivateKeySigner,
};
use anyhow::{Context, Result, anyhow, bail};
use reqwest::Client;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

use crate::{
    types::RpcBlock,
    utils::{hex_to_bytes, parse_u128_hex, parse_u64_hex, parse_u256_hex, wei_to_eth_string},
};

pub async fn rpc_call<T: DeserializeOwned>(
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

pub async fn eth_call(http: &Client, rpc_url: &str, to: Address, data: Bytes) -> Result<Vec<u8>> {
    let call_obj = json!({
        "to": to,
        "data": format!("0x{}", alloy::primitives::hex::encode(data.as_ref())),
    });

    let out_hex: String = rpc_call(http, rpc_url, "eth_call", json!([call_obj, "latest"]))
        .await
        .context("eth_call failed")?;

    hex_to_bytes(&out_hex)
}

pub async fn get_block_by_number(http: &Client, rpc_url: &str, block: &str) -> Result<RpcBlock> {
    rpc_call(http, rpc_url, "eth_getBlockByNumber", json!([block, false])).await
}

pub async fn get_block_number(http: &Client, rpc_url: &str) -> Result<u64> {
    let hex_num: String = rpc_call(http, rpc_url, "eth_blockNumber", json!([])).await?;
    parse_u64_hex(&hex_num)
}

pub async fn get_chain_id(http: &Client, rpc_url: &str) -> Result<u64> {
    let hex_num: String = rpc_call(http, rpc_url, "eth_chainId", json!([])).await?;
    parse_u64_hex(&hex_num)
}

pub async fn get_nonce(
    http: &Client,
    rpc_url: &str,
    addr: Address,
    block_tag: &str,
) -> Result<u64> {
    let hex_num: String = rpc_call(
        http,
        rpc_url,
        "eth_getTransactionCount",
        json!([addr, block_tag]),
    )
    .await?;
    parse_u64_hex(&hex_num)
}

pub async fn get_balance(http: &Client, rpc_url: &str, addr: Address) -> Result<U256> {
    let hex_num: String =
        rpc_call(http, rpc_url, "eth_getBalance", json!([addr, "latest"])).await?;
    parse_u256_hex(&hex_num)
}

pub async fn get_code(http: &Client, rpc_url: &str, addr: Address) -> Result<String> {
    rpc_call(http, rpc_url, "eth_getCode", json!([addr, "latest"]))
        .await
        .context("eth_getCode failed")
}

pub async fn send_raw_transaction(http: &Client, rpc_url: &str, raw_tx: &str) -> Result<String> {
    rpc_call(http, rpc_url, "eth_sendRawTransaction", json!([raw_tx]))
        .await
        .context("eth_sendRawTransaction failed")
}

pub async fn sweep_funding_wallet(
    http: &Client,
    rpc_url: &str,
    chain_id: u64,
    priority_fee: u128,
    funding_signer: &PrivateKeySigner,
    refund_address: Address,
) -> Result<()> {
    let funding = funding_signer.address();
    let funding_wallet = EthereumWallet::from(funding_signer.clone());

    let block = get_block_by_number(http, rpc_url, "latest").await?;
    let base_fee = parse_u128_hex(
        block
            .base_fee_per_gas
            .as_deref()
            .ok_or_else(|| anyhow!("latest block missing baseFeePerGas"))?,
    )?;
    let gas_price = base_fee + priority_fee;
    let gas_cost = U256::from(21_000u64) * U256::from(gas_price);

    let balance = get_balance(http, rpc_url, funding).await?;
    if balance <= gas_cost {
        println!(
            "Funding wallet balance ({} ETH) is at or below gas cost — nothing to sweep.",
            wei_to_eth_string(balance)
        );
        return Ok(());
    }

    let sweep_value = balance - gas_cost;
    println!(
        "Sweeping {} ETH from funding wallet {} to {}",
        wei_to_eth_string(sweep_value),
        funding,
        refund_address,
    );

    let nonce = get_nonce(http, rpc_url, funding, "pending").await?;

    let req = TransactionRequest::default()
        .from(funding)
        .to(refund_address)
        .nonce(nonce)
        .gas_limit(21_000)
        .gas_price(gas_price)
        .value(sweep_value);
    let mut req = req;
    req.chain_id = Some(chain_id);

    let raw = sign_request_to_hex(&funding_wallet, req).await?;
    let tx_hash = send_raw_transaction(http, rpc_url, &raw).await?;
    println!("Sweep tx: {}", tx_hash);

    loop {
        tokio::time::sleep(Duration::from_secs(3)).await;
        let receipt =
            rpc_call::<Value>(http, rpc_url, "eth_getTransactionReceipt", json!([tx_hash]))
                .await?;
        if !receipt.is_null() {
            println!("Sweep complete.");
            return Ok(());
        }
    }
}

pub(crate) async fn sign_request_to_hex(
    wallet: &EthereumWallet,
    req: TransactionRequest,
) -> Result<String> {
    let envelope = NetworkWallet::<Ethereum>::sign_request(wallet, req)
        .await
        .map_err(|e| anyhow!("failed to sign tx request: {e}"))?;

    Ok(format!("0x{}", hex::encode(envelope.encoded_2718())))
}

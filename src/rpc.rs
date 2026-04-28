use alloy::primitives::{Address, Bytes, U256};
use anyhow::{Context, Result, anyhow, bail};
use reqwest::Client;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

use crate::{
    types::RpcBlock,
    utils::{hex_to_bytes, parse_u64_hex, parse_u256_hex},
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

use alloy::primitives::{U256, hex};
use anyhow::Result;

pub fn parse_u128_hex(v: &str) -> Result<u128> {
    let stripped = v.strip_prefix("0x").unwrap_or(v);
    Ok(u128::from_str_radix(stripped, 16)?)
}

pub fn parse_u64_hex(v: &str) -> Result<u64> {
    let stripped = v.strip_prefix("0x").unwrap_or(v);
    Ok(u64::from_str_radix(stripped, 16)?)
}

pub fn parse_u256_hex(v: &str) -> Result<U256> {
    let stripped = v.strip_prefix("0x").unwrap_or(v);
    Ok(U256::from_str_radix(stripped, 16)?)
}

pub fn gwei_to_wei(gwei: u64) -> u128 {
    (gwei as u128) * 1_000_000_000u128
}

pub fn wei_to_eth_string(wei: U256) -> String {
    let scale = U256::from(1_000_000_000_000_000_000u128);
    let whole = wei / scale;
    let frac = (wei % scale) / U256::from(1_000_000_000_000u64);
    format!("{}.{:06}", whole, frac)
}

pub fn hex_to_bytes(s: &str) -> Result<Vec<u8>> {
    let stripped = s.strip_prefix("0x").unwrap_or(s);
    if stripped.is_empty() {
        return Ok(Vec::new());
    }
    Ok(hex::decode(stripped)?)
}

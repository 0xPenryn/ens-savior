use std::path::PathBuf;

use alloy::primitives::{Address, B256, U256};
use clap::Parser;
use serde::{Deserialize, Serialize};

use crate::constants::{DEFAULT_RELAY, DEFAULT_RPC, DEFAULT_SUBGRAPH};

#[derive(Debug, Parser)]
#[command(name = "ens-savior")]
#[command(about = "Recover ENS names from a compromised wallet via private bundle submission")]
pub struct Args {
    #[arg(long)]
    pub compromised_private_key: String,
    #[arg(long)]
    pub destination: String,
    #[arg(long, default_value = DEFAULT_RPC)]
    pub rpc_url: String,
    #[arg(long, default_value = DEFAULT_RELAY)]
    pub relay_url: String,
    #[arg(long, default_value = DEFAULT_SUBGRAPH)]
    pub subgraph_url: String,
    #[arg(long)]
    pub state_path: Option<PathBuf>,
    #[arg(long, default_value_t = 3)]
    pub priority_fee_gwei: u64,
    #[arg(long, default_value_t = 15)]
    pub safety_buffer_pct: u64,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct SessionState {
    pub compromised: Address,
    pub destination: Address,
    pub funding_private_key: String,
    pub completed: bool,
}

#[derive(Debug, Clone)]
pub enum RecoveryKind {
    BaseRegistrar2ld { token_id: U256 },
    NameWrapper { node: U256 },
    RegistryOwner { node: B256 },
}

impl RecoveryKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::BaseRegistrar2ld { .. } => "2LD .eth (BaseRegistrar ERC-721)",
            Self::NameWrapper { .. } => "Wrapped name (NameWrapper ERC-1155)",
            Self::RegistryOwner { .. } => "Registry owner-controlled name",
        }
    }
}

#[derive(Debug, Clone)]
pub struct PlannedNameTx {
    pub name: String,
    pub kind: RecoveryKind,
}

#[derive(Debug, Clone)]
pub struct FlashbotsBundle {
    pub tx_hashes: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RpcBlock {
    pub base_fee_per_gas: Option<u128>,
}

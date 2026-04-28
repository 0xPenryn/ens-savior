use std::path::PathBuf;

use alloy::primitives::{Address, B256, U256};
use clap::Parser;
use serde::{Deserialize, Serialize};

use crate::constants::{DEFAULT_RELAY, DEFAULT_RPC};

#[derive(Debug, Parser)]
#[command(name = "ens-savior")]
#[command(about = "Recover ENS names from a compromised wallet via private bundle submission")]
pub struct Args {
    /// Private key of the compromised wallet (hex). Mutually exclusive with --compromised-mnemonic.
    #[arg(long, conflicts_with = "compromised_mnemonic")]
    pub compromised_private_key: Option<String>,
    /// Seed phrase of the compromised wallet. Mutually exclusive with --compromised-private-key.
    #[arg(long, conflicts_with = "compromised_private_key")]
    pub compromised_mnemonic: Option<String>,
    /// BIP-44 derivation path index when using --compromised-mnemonic (default: 0).
    #[arg(long, default_value_t = 0)]
    pub mnemonic_index: u32,
    #[arg(long)]
    pub destination: String,
    #[arg(long, default_value = DEFAULT_RPC)]
    pub rpc_url: String,
    #[arg(long, default_value = DEFAULT_RELAY)]
    pub relay_url: String,
    /// Full subgraph URL. Mutually exclusive with --subgraph-api-key.
    #[arg(long, conflicts_with = "subgraph_api_key")]
    pub subgraph_url: Option<String>,
    /// The Graph API key; used to build the ENS subgraph URL automatically.
    #[arg(long, conflicts_with = "subgraph_url")]
    pub subgraph_api_key: Option<String>,
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
    pub base_fee_per_gas: Option<String>,
}

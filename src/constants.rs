use alloy::primitives::{Address, address};

pub const ENS_BASE_REGISTRAR: Address = address!("57f1887a8BF19b14fC0dF6Fd9B2acc9Af147eA85");
pub const ENS_NAME_WRAPPER: Address = address!("D4416b13d2b3a9aBae7AcD5D6C2BbDBE25686401");
pub const ENS_REGISTRY: Address = address!("00000000000C2E074eC69A0dFb2997BA6C7d2e1e");

pub const DEFAULT_RPC: &str = "https://ethereum.publicnode.com";
pub const DEFAULT_RELAY: &str = "https://relay.flashbots.net";

/// Builder names for eth_sendBundle multiplexing via relay.flashbots.net.
/// Source: https://raw.githubusercontent.com/flashbots/dowg/main/builder-registrations.json
pub const BUILDER_NAMES: &[&str] = &[
    "flashbots",
    "f1b.io",
    "rsync",
    "beaverbuild.org",
    "builder0x69",
    "Titan",
    "EigenPhi",
    "boba-builder",
    "Gambit Labs",
    "payload",
    "Loki",
    "BuildAI",
    "JetBuilder",
    "tbuilder",
    "penguinbuild",
    "bobthebuilder",
    "BTCS",
    "bloXroute",
    "Blockbeelder",
    "Quasar",
    "Eureka",
];
pub const ENS_SUBGRAPH_ID: &str = "5XqPmWe6gjyrJtFn9cLy237i4cWw2j9HcUJEXsP5qGtH";

/// Legacy hosted-service endpoint — no API key required but heavily rate-limited and may be unreliable.
/// Prefer --subgraph-api-key or --subgraph-url for production use.
pub const ENS_SUBGRAPH_FALLBACK_URL: &str =
    "https://api.thegraph.com/subgraphs/name/ensdomains/ens";

pub const GAS_DEAUTH: u64 = 80_000;
pub const GAS_FUND_TRANSFER: u64 = 21_000;
pub const GAS_BASE_REG_TRANSFER: u64 = 120_000;
pub const GAS_WRAPPER_TRANSFER: u64 = 180_000;
pub const GAS_REGISTRY_SET_OWNER: u64 = 80_000;

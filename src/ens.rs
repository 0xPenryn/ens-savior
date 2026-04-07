use std::collections::BTreeSet;

use alloy::{
    ens::namehash,
    primitives::{Address, U256},
};
use anyhow::{Result, bail};
use dialoguer::{MultiSelect, theme::ColorfulTheme};
use reqwest::Client;
use serde::Deserialize;
use serde_json::json;

use crate::{
    contracts,
    types::{PlannedNameTx, RecoveryKind},
};

pub async fn discover_names(
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
        let owner_on_registry = contracts::registry_owner(http, rpc_url, node).await?;
        let wrapped_owner = contracts::name_wrapper_owner(http, rpc_url, node_to_u256(node.0))
            .await
            .ok();

        if owner_on_registry == owner || wrapped_owner == Some(owner) || name.ends_with(".eth") {
            filtered.push(name);
        }
    }

    Ok(filtered)
}

pub fn select_names(names: &[String]) -> Result<Vec<String>> {
    let selection = MultiSelect::with_theme(&ColorfulTheme::default())
        .with_prompt("Select ENS names to recover")
        .items(names)
        .interact()?;

    Ok(selection
        .into_iter()
        .map(|idx| names[idx].clone())
        .collect())
}

pub async fn plan_name_recoveries(
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
            if let Ok(owner) = contracts::base_registrar_owner(http, rpc_url, token_id).await {
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
        if let Ok(owner) = contracts::name_wrapper_owner(http, rpc_url, node_u256).await {
            if owner == compromised {
                planned.push(PlannedNameTx {
                    name: name.clone(),
                    kind: RecoveryKind::NameWrapper { node: node_u256 },
                });
                continue;
            }
        }

        let registry_owner_addr = contracts::registry_owner(http, rpc_url, node).await?;
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

    if planned.is_empty() {
        bail!("Could not plan any recoverable transactions for selected names");
    }

    Ok(planned)
}

fn label_to_token_id(label: &str) -> U256 {
    let hash = alloy::primitives::keccak256(label.as_bytes());
    U256::from_be_slice(hash.as_slice())
}

fn node_to_u256(node: [u8; 32]) -> U256 {
    U256::from_be_slice(&node)
}

#[derive(Debug, Deserialize)]
struct SubgraphDomainsResp {
    data: Option<SubgraphData>,
    #[serde(default)]
    errors: Vec<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
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
struct WrappedDomain {
    name: Option<String>,
}

async fn discover_names_subgraph(
    http: &Client,
    subgraph_url: &str,
    owner: Address,
) -> Result<Vec<String>> {
    let owner_hex = format!("{owner:#x}");

    let query = r#"
        query($owner: String!) {
          domains(first: 1000, where: { owner: $owner }) {
            name
          }
          wrappedDomains(first: 1000, where: { owner: $owner }) {
            name
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
        .await?
        .error_for_status()?
        .json::<SubgraphDomainsResp>()
        .await?;

    if !resp.errors.is_empty() {
        anyhow::bail!("subgraph returned errors: {}", serde_json::to_string(&resp.errors)?);
    }

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
        }
    }

    Ok(out.into_iter().collect())
}

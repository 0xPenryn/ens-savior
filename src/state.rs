use std::{fs, path::PathBuf, str::FromStr};

use alloy::{
    primitives::{Address, hex},
    signers::local::PrivateKeySigner,
};
use anyhow::{Result, anyhow, bail};

use crate::types::{Args, SessionState};

pub fn parse_signer(input: &str) -> Result<PrivateKeySigner> {
    let normalized = input.strip_prefix("0x").unwrap_or(input);
    let with_prefix = format!("0x{}", normalized);
    Ok(PrivateKeySigner::from_str(&with_prefix)?)
}

pub fn resolve_state_path(
    args: &Args,
    compromised: Address,
    destination: Address,
) -> Result<PathBuf> {
    if let Some(path) = &args.state_path {
        return Ok(path.clone());
    }

    let mut base = dirs::config_dir().ok_or_else(|| anyhow!("unable to resolve config dir"))?;
    base.push("ens-savior");
    fs::create_dir_all(&base)?;
    base.push(format!("{}_{}.toml", compromised, destination));
    Ok(base)
}

pub fn load_or_create_session(
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

pub fn persist_completed(state_path: &PathBuf, mut session: SessionState) -> Result<()> {
    session.completed = true;
    fs::write(state_path, toml::to_string_pretty(&session)?)?;
    Ok(())
}

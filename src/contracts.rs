use alloy::{
    primitives::{Address, B256, Bytes, U256},
    sol,
    sol_types::SolCall,
};
use anyhow::{Result, anyhow};
use reqwest::Client;

use crate::{
    constants::{ENS_BASE_REGISTRAR, ENS_NAME_WRAPPER, ENS_REGISTRY},
    rpc,
};

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

pub fn build_base_registrar_transfer(
    from: Address,
    to: Address,
    token_id: U256,
) -> (Address, Bytes) {
    let call = IBaseRegistrar::transferFromCall {
        from,
        to,
        tokenId: token_id,
    };
    (ENS_BASE_REGISTRAR, Bytes::from(call.abi_encode()))
}

pub fn build_wrapper_transfer(from: Address, to: Address, node: U256) -> (Address, Bytes) {
    let call = INameWrapper::safeTransferFromCall {
        from,
        to,
        id: node,
        amount: U256::from(1),
        data: Bytes::new(),
    };
    (ENS_NAME_WRAPPER, Bytes::from(call.abi_encode()))
}

pub fn build_registry_set_owner(to: Address, node: B256) -> (Address, Bytes) {
    let call = IENSRegistry::setOwnerCall { node, owner: to };
    (ENS_REGISTRY, Bytes::from(call.abi_encode()))
}

pub async fn base_registrar_owner(http: &Client, rpc_url: &str, token_id: U256) -> Result<Address> {
    let call = IBaseRegistrar::ownerOfCall { id: token_id };
    let output = rpc::eth_call(
        http,
        rpc_url,
        ENS_BASE_REGISTRAR,
        Bytes::from(call.abi_encode()),
    )
    .await?;
    IBaseRegistrar::ownerOfCall::abi_decode_returns(&output)
        .map_err(|e| anyhow!("failed to decode base registrar ownerOf: {e}"))
}

pub async fn name_wrapper_owner(http: &Client, rpc_url: &str, node: U256) -> Result<Address> {
    let call = INameWrapper::ownerOfCall { id: node };
    let output = rpc::eth_call(
        http,
        rpc_url,
        ENS_NAME_WRAPPER,
        Bytes::from(call.abi_encode()),
    )
    .await?;
    INameWrapper::ownerOfCall::abi_decode_returns(&output)
        .map_err(|e| anyhow!("failed to decode name wrapper ownerOf: {e}"))
}

pub async fn registry_owner(http: &Client, rpc_url: &str, node: B256) -> Result<Address> {
    let call = IENSRegistry::ownerCall { node };
    let output = rpc::eth_call(http, rpc_url, ENS_REGISTRY, Bytes::from(call.abi_encode())).await?;
    IENSRegistry::ownerCall::abi_decode_returns(&output)
        .map_err(|e| anyhow!("failed to decode ens registry owner: {e}"))
}

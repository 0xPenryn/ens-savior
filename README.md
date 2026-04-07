# ens-savior

`ens-savior` is a Rust CLI for recovering ENS names from a compromised private key by submitting a private Flashbots bundle.

It is designed for situations where the compromised wallet is actively monitored and public mempool transactions are likely to be front-run.

## What It Does

- Accepts:
  - compromised wallet private key
  - destination wallet address
- Creates and persists a temporary funding wallet (reused across attempts)
- Detects if EIP-7702 deauthorization is needed and includes deauth tx only when required
- Discovers ENS names owned by the compromised account
- Lets you select which names to recover
- Plans recovery transactions based on ownership path:
  - `.eth` 2LD via BaseRegistrar (`transferFrom`)
  - wrapped names via NameWrapper (`safeTransferFrom`)
  - direct registry ownership via ENS Registry (`setOwner`)
- Estimates required funding
- Waits for funding wallet balance to reach required amount
- Simulates and submits all signed txs in one Flashbots bundle
- Re-submits per block until inclusion is detected

## Requirements

- Rust toolchain (stable)
- Network access to:
  - Ethereum RPC endpoint
  - Flashbots relay
  - ENS subgraph

## Build

```bash
cargo build --release
```

Binary path:

```bash
./target/release/ens-savior
```

## Usage

```bash
cargo run -- \
  --compromised-private-key 0xYOUR_PRIVATE_KEY \
  --destination 0xDESTINATION_ADDRESS
```

Or with explicit endpoints:

```bash
cargo run -- \
  --compromised-private-key 0xYOUR_PRIVATE_KEY \
  --destination 0xDESTINATION_ADDRESS \
  --rpc-url https://rpc.flashbots.net/fast \
  --relay-url https://relay.flashbots.net \
  --subgraph-url https://api.thegraph.com/subgraphs/name/ensdomains/ens
```

## CLI Options

- `--compromised-private-key <HEX>`: private key for compromised wallet
- `--destination <ADDRESS>`: destination wallet address for recovered names
- `--rpc-url <URL>`: Ethereum RPC URL
  - default: `https://rpc.flashbots.net/fast`
- `--relay-url <URL>`: Flashbots relay URL
  - default: `https://relay.flashbots.net`
- `--subgraph-url <URL>`: ENS subgraph URL used for discovery
  - default: `https://api.thegraph.com/subgraphs/name/ensdomains/ens`
- `--state-path <PATH>`: optional custom session state file path
- `--priority-fee-gwei <N>`: tip used in bundle transactions
  - default: `3`
- `--safety-buffer-pct <N>`: gas funding safety buffer percentage
  - default: `15`

## Session State

By default, session state is stored at:

```text
$XDG_CONFIG_HOME/ens-savior/<compromised>_<destination>.toml
```

On macOS this is typically under:

```text
~/Library/Application Support/ens-savior/
```

The state file contains the generated funding wallet private key and completion flag.

## Recovery Flow

1. Start tool with compromised key + destination address
2. Tool prints compromised, destination, and funding wallet addresses
3. Tool discovers recoverable ENS names and prompts for selection
4. Tool estimates required funding and prompts to continue
5. You fund the generated funding wallet
6. Tool waits for balance, then simulates and submits Flashbots bundle
7. Tool re-submits each new block until included

## Security Notes

- Treat all keys as highly sensitive
- Run from a secure environment
- Prefer a fresh machine/session with minimal background software
- Do not reuse compromised wallets after recovery
- Verify destination address before running

## Disclaimer

This software is provided as-is, with no warranty. You are fully responsible for operational security, key management, and transaction outcomes.

# ens-savior

`ens-savior` is a Rust CLI for recovering ENS names from a compromised private key by submitting a private Flashbots bundle.

It is designed for situations where the compromised wallet is actively monitored and public mempool transactions are likely to be front-run.

## What It Does

- Accepts:
  - compromised wallet private key
  - destination wallet address
- Creates and persists a temporary funding wallet (reused across attempts)
- Detects if EIP-7702 deauthorization is needed and includes deauth tx only when required
- Discovers ENS names owned by the compromised account via The Graph ENS subgraph
- Lets you select which names to recover
- Plans recovery transactions based on ownership path:
  - `.eth` 2LD via BaseRegistrar (`transferFrom`)
  - wrapped names via NameWrapper (`safeTransferFrom`)
  - direct registry ownership via ENS Registry (`setOwner`)
- Estimates required funding
- Waits for funding wallet balance to reach required amount
- Simulates the bundle via `eth_callBundle` and prints per-tx gas usage
- Submits via `eth_sendBundle` to `relay.flashbots.net`, multiplexed across all registered builders
- Re-submits per block until inclusion is detected

## Requirements

- Rust toolchain (stable)
- A Graph API key (free at [thegraph.com/studio/apikeys](https://thegraph.com/studio/apikeys/))
- Network access to an Ethereum RPC endpoint and the Flashbots relay

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
  --destination 0xDESTINATION_ADDRESS \
  --subgraph-api-key YOUR_GRAPH_API_KEY
```

Or with a full custom subgraph URL:

```bash
cargo run -- \
  --compromised-private-key 0xYOUR_PRIVATE_KEY \
  --destination 0xDESTINATION_ADDRESS \
  --subgraph-url https://gateway.thegraph.com/api/YOUR_KEY/subgraphs/id/5XqPmWe6gjyrJtFn9cLy237i4cWw2j9HcUJEXsP5qGtH
```

## CLI Options

| Flag | Description | Default |
|------|-------------|---------|
| `--compromised-private-key <HEX>` | Private key for the compromised wallet | required |
| `--destination <ADDRESS>` | Destination wallet address for recovered names | required |
| `--subgraph-api-key <KEY>` | The Graph API key — builds the ENS subgraph URL automatically | required (or use `--subgraph-url`) |
| `--subgraph-url <URL>` | Full ENS subgraph URL (alternative to `--subgraph-api-key`) | — |
| `--rpc-url <URL>` | Ethereum JSON-RPC endpoint | `https://ethereum.publicnode.com` |
| `--relay-url <URL>` | Flashbots relay URL for simulation and bundle submission | `https://relay.flashbots.net` |
| `--state-path <PATH>` | Custom session state file path | see below |
| `--priority-fee-gwei <N>` | Priority fee tip used in bundle transactions | `3` |
| `--safety-buffer-pct <N>` | Gas funding safety buffer percentage | `15` |

`--subgraph-api-key` and `--subgraph-url` are mutually exclusive; one must be provided.

## ENS Subgraph

Name discovery uses the ENS subgraph on The Graph Network (subgraph ID `5XqPmWe6gjyrJtFn9cLy237i4cWw2j9HcUJEXsP5qGtH`). A free API key can be obtained at [thegraph.com/studio/apikeys](https://thegraph.com/studio/apikeys/).

## Bundle Submission

Bundles are signed per the Flashbots authentication spec (EIP-191 personal sign of `keccak256(body)` as a hex string) and submitted as a single `eth_sendBundle` request to the Flashbots relay. The `builders` parameter is set to all builders registered in the [Flashbots DOWG builder registry](https://github.com/flashbots/dowg/blob/main/builder-registrations.json), so the relay multiplexes the bundle to every registered builder in one request.

## Session State

By default, session state is stored at:

```text
$XDG_CONFIG_HOME/ens-savior/<compromised>_<destination>.toml
```

On macOS this is typically:

```text
~/Library/Application Support/ens-savior/
```

The state file contains the generated funding wallet private key and completion flag. If you re-run the tool with the same compromised/destination pair, the same funding wallet is reused.

## Recovery Flow

1. Run the tool with your compromised key, destination, and subgraph API key
2. The tool prints compromised, destination, and funding wallet addresses
3. ENS names owned by the compromised wallet are discovered and presented for selection
4. The tool estimates required funding and prompts to continue
5. Fund the generated funding wallet with the displayed amount
6. The tool simulates the bundle and prints per-tx gas usage, then submits to the Flashbots relay
7. The bundle is re-submitted each block until inclusion is confirmed

## Security Notes

- Treat all keys as highly sensitive
- Run from a secure environment
- Prefer a fresh machine/session with minimal background software
- Do not reuse the compromised wallet after recovery
- Verify the destination address carefully before running

## Disclaimer

This software is provided as-is, with no warranty. You are fully responsible for operational security, key management, and transaction outcomes.

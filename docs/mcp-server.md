# Maya2C MCP server

`maya2c-mcp` lets an AI assistant read a Maya2C node and draft transfers.
It cannot sign or send anything. A person signs drafts with their own wallet.
Decision record: [ADR-084](adr/ADR-084-mcp-read-and-draft-only.md).

## Build

```
cargo build --release -p maya2c-mcp
```

The binary is `target/release/maya2c-mcp` (`maya2c-mcp.exe` on Windows).

## Connect

Point the server at a node's RPC address with `--rpc HOST:PORT`.

**Claude Code**

```
claude mcp add maya2c -- /path/to/maya2c-mcp --rpc 127.0.0.1:8545
```

**Claude Desktop** (`claude_desktop_config.json`)

```json
{
  "mcpServers": {
    "maya2c": {
      "command": "C:\\path\\to\\maya2c-mcp.exe",
      "args": ["--rpc", "127.0.0.1:8545"]
    }
  }
}
```

Restart the client after editing the config.

## Tools

| Tool | What it does |
|---|---|
| `get_height` | Height, chain id and genesis (the chain tag signatures commit to) |
| `get_block` | Header, transaction count and txids for a height |
| `get_tx` | Finds a txid inside the block at a given height |
| `get_balance` | Balance (base units) and next nonce; `maya1...` or hex |
| `get_validators` | Validator set and staking record |
| `get_supply` | Total, circulating and burned supply |
| `estimate_fee` | Minimum fee for a standard transfer |
| `draft_transfer` | Unsigned transfer (hex), fee, total, balance check and a summary |
| `decode_transaction` | Decodes a hex transaction with the node's decoder |
| `review_intent` | Compares claimed and simulated effects; lists warnings |
| `explain_error` | Plain-language meaning of a node error kind, with its spec rule |

Amounts in `draft_transfer` are coins, up to 24 decimal places (`"1.5"`).
Every other amount is in base units (24 decimals per coin).

## Using a draft

1. The assistant calls `draft_transfer` and shows you the `summary`.
2. Check the recipient, the amount, the fee and `network`.
3. Sign `unsigned_tx_hex` with your own wallet, for the `chain_tag` shown.
   Submit it there.

If `network` is null, the node did not name its chain. Compare `chain_tag`
with the network you intend to use before signing.

## Security model

- **No keys.** The server never reads, stores or generates a key or seed.
- **No write path.** The node is reached through six read methods, checked
  against one allowlist at the single point where requests leave the process.
  Writes such as `send_raw_transaction` are not reachable from any tool,
  whether the node is local or remote.
- **Drafts are unsigned.** A draft carries no signature. The wallet adds the
  sender's public key and signature, so the server cannot authorise a payment.
- **Inputs are checked before any request.** Addresses, amounts (at most 24 decimals, never
  rounded) and request lines (at most 256 KiB) are validated before anything
  is sent to the node.
- **Rate limit.** Requests are limited to the node's own default per-client
  rate (50 per second, burst 100).
- **Errors do not leak.** Node error messages are passed through; the server
  writes nothing to stderr and logs no request arguments.

Known limits: `get_tx` needs a block height because the node has no lookup by
id. The 256 KiB request cap and the rate numbers are chosen, not measured.

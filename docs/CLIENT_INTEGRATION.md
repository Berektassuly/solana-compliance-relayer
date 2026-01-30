# Client Integration

This document covers SDK integration notes and CLI tools for developers building clients for the Solana Compliance Relayer.

---

## Table of Contents

- [SDK Integration Notes](#sdk-integration-notes)
- [CLI Tools](#cli-tools)
- [Signing Implementation Guide](#signing-implementation-guide)

---

## SDK Integration Notes

When integrating with the API (WASM, mobile, or server SDKs), follow these guidelines to ensure proper request handling.

### Nonce Generation

- **Generate a unique nonce per request**
- Use UUID v4 or (recommended) UUID v7 for time-ordered uniqueness
- The nonce must be **32–64 characters**, alphanumeric with optional hyphens
- The nonce must be included in **both** the request body **and** the signed message

### Idempotency

- **Send the same nonce as `Idempotency-Key`** when retrying the same logical request
- On timeout or network error, retry with the **same nonce and signature**
- The server returns the original response (200 OK) instead of creating a duplicate transfer

### Signing Message Format

The message you sign must use this exact format (server logic: `src/domain/types.rs` — `create_signing_message`):

```
{from}:{to}:{amount}:{mint}:{nonce}
```

| Field | Description |
|-------|-------------|
| `from` | Sender wallet public key (Base58) |
| `to` | Recipient wallet public key (Base58) |
| `amount` | For **public**: decimal string of the u64 amount (e.g. `1000000000`). For **confidential**: literal `confidential` |
| `mint` | For **SOL**: literal `SOL` (do **not** use the string `null`). For SPL tokens: mint address (Base58) |
| `nonce` | The unique nonce value |

> **Critical:** When `token_mint` is null (native SOL transfer), the **mint** component in the message must be the literal string `SOL`. The server uses `token_mint.as_deref().unwrap_or("SOL")`, so `null` in the message would cause signature verification to fail (403).

### Example Integration Flow

```
1. Generate unique nonce (UUID v7; 32–64 chars, alphanumeric + hyphens)
2. Construct signing message: "{from}:{to}:{amount}:{mint}:{nonce}"
   - amount: decimal string (e.g. "1000000000") or "confidential"
   - mint: "SOL" for native SOL, or mint address Base58
3. Sign message with Ed25519 (client-side); encode signature as Base58
4. POST /transfer-requests with:
   - Request body: from_address, to_address, transfer_details (type + amount or confidential proofs), token_mint (null for SOL), signature, nonce
   - Idempotency-Key header = nonce (optional but recommended)
5. On success (200): store transfer id from response body
6. On timeout: retry with SAME nonce and signature (idempotent)
7. Poll GET /transfer-requests/{id} for blockchain_status (e.g. pending_submission → submitted → confirmed)
```

### WASM Integration

If you use a Rust-compiled WebAssembly signer (e.g. `ed25519-dalek` or equivalent), the message format must match the server exactly:

```javascript
// Example: Message construction for WASM or native signer
// amount must be the decimal string (e.g. "1000000000"); mint must be "SOL" for native SOL
const message = `${fromAddress}:${toAddress}:${amount}:${mint}:${nonce}`;
// Sign message with Ed25519 (WASM or tweetnacl), then Base58-encode the 64-byte signature
const signature = signMessage(privateKeyBytes, new TextEncoder().encode(message));
```

---

## CLI Tools

The project includes CLI utilities for generating valid transfer requests with proper Ed25519 signatures.

### generate_transfer_request

Generates a complete, signed transfer request and outputs a ready-to-use curl command. Uses a dev keypair (or override via code).

**Usage:**

```bash
# Generate a public SOL transfer (1 SOL)
cargo run --bin generate_transfer_request

# Generate a confidential transfer with real ZK proofs
cargo run --bin generate_transfer_request -- --confidential
```

**Example Output (Public Transfer):**

The tool prints the keypair, nonce, signing message (matching server format `{from}:{to}:{amount}:SOL:{nonce}`), then a ready-to-use curl command:

```
Generated Keypair:
   Public Key (from_address): <Base58 pubkey>
   Private Key (keep safe):   [32 bytes...]

--------------------------------------------------

Nonce: "<UUID v7>"
Signing Message: "<from>:<to>:1000000000:SOL:<nonce>"

Generated curl command:

curl -X POST 'http://localhost:3000/transfer-requests' \
  -H 'Content-Type: application/json' \
  -H 'Idempotency-Key: <nonce>' \
  -d '{
    "from_address": "<sender Base58>",
    "to_address": "<recipient Base58>",
    "transfer_details": { "type": "public", "amount": 1000000000 },
    "token_mint": null,
    "signature": "<Base58 Ed25519 signature>",
    "nonce": "<same nonce as in signing message>"
  }'
```

**Confidential Transfer Mode:**

When using `--confidential`, the tool:

1. Generates ElGamal and AES encryption keys
2. Simulates an account with 10 SOL balance
3. Produces real ZK proofs (equality, ciphertext validity, range)
4. Outputs a complete request with Base64-encoded proof data

```bash
cargo run --bin generate_transfer_request -- --confidential
```

This outputs:

- Equality proof (~200 bytes)
- Ciphertext validity proof (~400 bytes)
- Range proof (~700 bytes)
- New decryptable balance (36 bytes)

### setup_and_generate

Creates real on-chain confidential transfer state on the **zk-edge** testnet (`https://zk-edge.surfnet.dev:8899` — see `RPC_URL` in `src/bin/setup_and_generate.rs`), then generates valid ZK proofs and a `TransferRequest` JSON for the relayer.

**Use this for end-to-end testing of Token-2022 confidential transfers.**

**Steps (as implemented in the binary):**

1. Create Token-2022 mint with ConfidentialTransfer extension
2. Create and configure sender token account for confidential transfers
3. Mint tokens to sender (public balance)
4. Deposit: public → confidential pending balance
5. Apply pending balance → available balance
6. Generate ZK proofs using real on-chain state
7. Output curl command for `POST /transfer-requests`

**Usage:**

```bash
cargo run --bin setup_and_generate
```

No CLI arguments; configuration is via constants and RPC in the binary.

**Requirements:**

- Airdrop-funded authority on zk-edge testnet
- Network access to `https://zk-edge.surfnet.dev:8899`

**Output includes:**

- Mint address, source and destination ATAs
- Ready-to-use `curl` command for `POST /transfer-requests`

---

## Signing Implementation Guide

### Rust (ed25519-dalek)

Message format must match the server: `{from}:{to}:{amount}:{mint}:{nonce}`. For SOL transfers use mint `"SOL"`; for public transfers amount is the u64 (formatted as decimal string); for confidential use the literal string `"confidential"` as the amount component.

```rust
use ed25519_dalek::{Keypair, Signer};

fn sign_transfer_request(
    keypair: &Keypair,
    from: &str,
    to: &str,
    amount_part: &str,  // decimal string e.g. "1000000000", or "confidential"
    mint: &str,         // "SOL" for native SOL, or mint address Base58
    nonce: &str,
) -> String {
    let message = format!("{}:{}:{}:{}:{}", from, to, amount_part, mint, nonce);
    let signature = keypair.sign(message.as_bytes());
    bs58::encode(signature.to_bytes()).into_string()
}

// Example: public SOL transfer
// sign_transfer_request(&keypair, &from, &to, "1000000000", "SOL", &nonce);
// Example: confidential transfer
// sign_transfer_request(&keypair, &from, &to, "confidential", &mint_address, &nonce);
```

### JavaScript/TypeScript (tweetnacl)

Use UTF-8 message bytes; amount must be the decimal string (e.g. `"1000000000"`) or `"confidential"`; mint must be `"SOL"` for native SOL. `secretKey` is the 64-byte Ed25519 key (32-byte seed + 32-byte public key in tweetnacl).

```typescript
import nacl from 'tweetnacl';
import bs58 from 'bs58';

function signTransferRequest(
    secretKey: Uint8Array,
    from: string,
    to: string,
    amount: string,   // "1000000000" or "confidential"
    mint: string,     // "SOL" or mint address Base58
    nonce: string
): string {
    const message = `${from}:${to}:${amount}:${mint}:${nonce}`;
    const messageBytes = new TextEncoder().encode(message);
    const signature = nacl.sign.detached(messageBytes, secretKey);
    return bs58.encode(signature);
}
```

### Python (ed25519)

Message must be UTF-8; amount is the decimal string (e.g. `"1000000000"`) or `"confidential"`; mint is `"SOL"` for native SOL. Use the `ed25519` package (e.g. `pip install ed25519`); private_key is 32 bytes.

```python
import ed25519
import base58

def sign_transfer_request(
    private_key: bytes,
    from_addr: str,
    to_addr: str,
    amount: str,   # "1000000000" or "confidential"
    mint: str,     # "SOL" or mint address Base58
    nonce: str
) -> str:
    message = f"{from_addr}:{to_addr}:{amount}:{mint}:{nonce}"
    signing_key = ed25519.SigningKey(private_key)
    signature = signing_key.sign(message.encode("utf-8"))
    return base58.b58encode(signature).decode("ascii")
```

---

## Error Handling

### Common Error Responses

API errors return JSON: `{ "error": { "type": "<type>", "message": "<message>" } }` (see `src/api/handlers.rs` and `src/domain/error.rs`).

| HTTP | `error.type` | Cause | Resolution |
|------|----------------|-------|------------|
| `403` | `authorization_error` | Signature verification failed, invalid encoding, or wrong key. Message may be "Signature verification failed: ...", "Invalid signature encoding: ...", "Invalid from_address length: ...", etc. | Ensure signing message is exactly `{from}:{to}:{amount}:{mint}:{nonce}`; use `SOL` for mint when native SOL; amount as decimal string or `confidential`. |
| `200` | — | **Idempotent duplicate:** same `(from_address, nonce)` already submitted | Response body is the existing transfer request. No new resource created. |
| `409` | `duplicate` | Database duplicate (e.g. nonce conflict on insert) | Rare if you use idempotency; otherwise use a new nonce. |
| `400` | `validation_error` | Idempotency-Key header does not match body `nonce`. Message: "Invalid field 'Idempotency-Key': Header must match body nonce" | Set `Idempotency-Key` to the same value as body `nonce`, or omit the header. |
| `400` | `validation_error` | Missing/invalid fields (e.g. nonce length, amount zero) | Fix request body per API_REFERENCE.md. |
| `200` | — | **Transfer blocked:** sender or recipient in blocklist, or compliance rejected | Response body has `compliance_status: "rejected"` and `blockchain_status: "failed"`. Inspect `blockchain_last_error` for reason. |

### Retry Strategy

```
1. On 429 (rate limit): Wait for Retry-After (seconds) header, then retry
2. On 5xx (server error): Exponential backoff (1s, 2s, 4s, 8s, max 60s)
3. On timeout: Retry with SAME nonce and signature (idempotent; server returns 200 with existing request)
4. On 200 with blockchain_status pending_submission/submitted: Poll GET /transfer-requests/{id} every 5s
```

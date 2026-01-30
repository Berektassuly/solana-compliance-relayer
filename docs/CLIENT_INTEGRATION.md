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

The message you sign must use this exact format:

```
{from}:{to}:{amount}:{mint}:{nonce}
```

| Field | Description |
|-------|-------------|
| `from` | Sender wallet public key (Base58) |
| `to` | Recipient wallet public key (Base58) |
| `amount` | For public: numeric amount. For confidential: literal `confidential` |
| `mint` | For SOL: literal `SOL`. For SPL tokens: mint address (Base58) |
| `nonce` | The unique nonce value |

### Example Integration Flow

```
1. Generate unique nonce (UUID v7)
2. Construct signing message: "{from}:{to}:{amount}:{mint}:{nonce}"
3. Sign message with Ed25519 (client-side)
4. POST /transfer-requests with:
   - Request body (from, to, amount, mint, signature, nonce)
   - Idempotency-Key header = nonce
5. On success: store transfer ID
6. On timeout: retry with SAME nonce and signature
7. Poll GET /transfer-requests/{id} for status updates
```

### WASM Integration

The frontend uses Rust-compiled WebAssembly for client-side signing:

```javascript
// Example: Using WASM signer in browser
import init, { sign_message } from 'wasm-signer';

await init();

const message = `${fromAddress}:${toAddress}:${amount}:SOL:${nonce}`;
const signature = sign_message(privateKeyBytes, message);
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

```
Generated Keypair:
   Public Key (from_address): 7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU
   Private Key (keep safe):   [32 bytes...]

--------------------------------------------------

Nonce: "019470a4-7e7c-7d3e-8f1a-2b3c4d5e6f7a"
Signing Message: "7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU:randomDest...:1000000000:SOL:019470a4-7e7c-7d3e-8f1a-2b3c4d5e6f7a"

Generated curl command:

curl -X POST 'http://localhost:3000/transfer-requests' \
  -H 'Content-Type: application/json' \
  -H 'Idempotency-Key: 019470a4-7e7c-7d3e-8f1a-2b3c4d5e6f7a' \
  -d '{
    "from_address": "7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU",
    "to_address": "randomDestination...",
    "transfer_details": {
      "type": "public",
      "amount": 1000000000
    },
    "token_mint": null,
    "signature": "BASE58_SIGNATURE_OVER_MESSAGE_WITH_NONCE",
    "nonce": "019470a4-7e7c-7d3e-8f1a-2b3c4d5e6f7a"
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

Creates real on-chain confidential transfer state on the **zk-edge** testnet (`https://zk-edge.surfnet.dev:8899`), then generates valid ZK proofs and a `TransferRequest` JSON for the relayer.

**Use this for end-to-end testing of Token-2022 confidential transfers.**

**Steps:**

1. Create mint
2. Create source/dest token accounts
3. Mint tokens
4. Deposit to confidential balance
5. Apply pending balance
6. Generate ZK proofs
7. Output curl command

**Usage:**

```bash
cargo run --bin setup_and_generate
```

**Requirements:**

- Airdrop-funded authority on zk-edge testnet
- Network access to `https://zk-edge.surfnet.dev:8899`

**Output includes:**

- Mint address
- Source and destination ATAs
- Ready-to-use `curl` command for `POST /transfer-requests`

---

## Signing Implementation Guide

### Rust (ed25519-dalek)

```rust
use ed25519_dalek::{Keypair, Signer};

fn sign_transfer_request(
    keypair: &Keypair,
    from: &str,
    to: &str,
    amount: u64,
    mint: &str,
    nonce: &str,
) -> String {
    let message = format!("{}:{}:{}:{}:{}", from, to, amount, mint, nonce);
    let signature = keypair.sign(message.as_bytes());
    bs58::encode(signature.to_bytes()).into_string()
}
```

### JavaScript/TypeScript (tweetnacl)

```typescript
import nacl from 'tweetnacl';
import bs58 from 'bs58';

function signTransferRequest(
    secretKey: Uint8Array,
    from: string,
    to: string,
    amount: string,
    mint: string,
    nonce: string
): string {
    const message = `${from}:${to}:${amount}:${mint}:${nonce}`;
    const messageBytes = new TextEncoder().encode(message);
    const signature = nacl.sign.detached(messageBytes, secretKey);
    return bs58.encode(signature);
}
```

### Python (ed25519)

```python
import ed25519
import base58

def sign_transfer_request(
    private_key: bytes,
    from_addr: str,
    to_addr: str,
    amount: str,
    mint: str,
    nonce: str
) -> str:
    message = f"{from_addr}:{to_addr}:{amount}:{mint}:{nonce}"
    signing_key = ed25519.SigningKey(private_key)
    signature = signing_key.sign(message.encode())
    return base58.b58encode(signature).decode()
```

---

## Error Handling

### Common Error Responses

| Error | Cause | Resolution |
|-------|-------|------------|
| `Invalid signature` | Message format mismatch or wrong key | Verify signing message format includes nonce |
| `Duplicate nonce` | Nonce already used | Generate new unique nonce |
| `Idempotency-Key mismatch` | Header doesn't match body nonce | Ensure header equals body `nonce` field |
| `Address blocked` | Sender or recipient in blocklist | Use different address or request review |
| `Compliance rejected` | Range Protocol flagged address | Address has high risk score |

### Retry Strategy

```
1. On 429 (rate limit): Wait for X-RateLimit-Reset, then retry
2. On 5xx (server error): Exponential backoff (1s, 2s, 4s, 8s, max 60s)
3. On timeout: Retry with SAME nonce and signature (idempotent)
4. On 200 with pending status: Poll GET endpoint every 5s
```

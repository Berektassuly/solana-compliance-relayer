# API Reference

Complete API reference for the Solana Compliance Relayer v0.3.0.

---

## Table of Contents

- [Security & Authentication](#security--authentication)
- [Data Type Specifications](#data-type-specifications)
- [Core Endpoints](#core-endpoints)
- [Admin Endpoints](#admin-endpoints)
- [Compliance Endpoints](#compliance-endpoints)
- [Webhook Endpoints](#webhook-endpoints)
- [Health Endpoints](#health-endpoints)
- [Signing Message Format](#signing-message-format)
- [Request Uniqueness (Nonce & Idempotency)](#request-uniqueness-nonce--idempotency)
- [Response Codes](#response-codes)
- [Rate Limiting](#rate-limiting)

---

## Security & Authentication

### Client Signature Verification

All `POST /transfer-requests` submissions require a valid **Ed25519 signature** proving ownership of the sender wallet. The signature is verified server-side before any processing occurs.

### Webhook Integrity

Webhook endpoints validate incoming requests to prevent spoofing:

| Provider | Header | Validation |
|----------|--------|------------|
| **Helius** | `Authorization` | Compared against `HELIUS_WEBHOOK_SECRET` env var. If configured and header missing or mismatched → `401 Unauthorized`. |
| **QuickNode** | `x-qn-signature` or `Authorization` | Compared against `QUICKNODE_WEBHOOK_SECRET` env var. **Current behavior:** When the secret is set, missing or mismatched headers are logged but **not rejected** (validation is lenient for debugging). |

> [!NOTE]
> **Helius:** If `HELIUS_WEBHOOK_SECRET` is set and the `Authorization` header is missing or does not match, the request is rejected with `401 Unauthorized`.
>
> **QuickNode:** If `QUICKNODE_WEBHOOK_SECRET` is set, the relayer currently accepts requests even when the header is missing or mismatched (logs only). Restrict access at the network level if strict verification is required.

### Replay Attack Protection

The server **tracks all nonces** in the database. Each `(from_address, nonce)` pair can only be used once. Duplicate submissions return the existing request (HTTP 200) rather than creating a duplicate.

> [!CAUTION]
> **Admin Endpoints Lack Application-Level Authentication**
>
> The `/admin/*` routes currently have **no authentication middleware**. They are intended for internal operations only.
>
> **Deployment Requirement:** Restrict access at the network level using:
> - VPN-only access
> - Internal network isolation
> - Reverse proxy with IP allowlisting
> - Kubernetes NetworkPolicy
>
> **Do NOT expose `/admin/*` routes to the public internet.**

---

## Data Type Specifications

### Amount Field Requirements

> [!IMPORTANT]
> The `amount` field **must be an unsigned 64-bit integer** (`u64`). Floating-point numbers are **strictly forbidden** and will cause JSON deserialization to fail with a `400 Bad Request`.

| Asset | Decimals | 1 Unit in Atomic | JSON Example |
|-------|----------|------------------|--------------|
| **SOL** | 9 | 1 SOL = `1,000,000,000` lamports | `"amount": 1000000000` |
| **USDC** | 6 | 1 USDC = `1,000,000` | `"amount": 1000000` |
| **USDT** | 6 | 1 USDT = `1,000,000` | `"amount": 1000000` |

**Validation:** For public transfers, `amount` must be **greater than 0** (zero is rejected with `400 Bad Request`).

**Incorrect (will fail):**
```json
{ "amount": 1.5 }
```
```json
{ "amount": 0 }
```

**Correct:**
```json
{ "amount": 1500000000 }
```

### Address Fields

All address fields (`from_address`, `to_address`, `token_mint`) must be **Base58-encoded Solana public keys** (32 bytes → 43-44 characters).

### Confidential Transfer Proofs

For confidential transfers, proof fields must be **Base64-encoded** strings:

| Field | Description | Generation |
|-------|-------------|------------|
| `equality_proof` | CiphertextCommitmentEqualityProofData | Client SDK |
| `ciphertext_validity_proof` | BatchedGroupedCiphertext3HandlesValidityProofData | Client SDK |
| `range_proof` | BatchedRangeProofU128Data | Client SDK |
| `new_decryptable_available_balance` | AES-encrypted balance | Client SDK |

> [!WARNING]
> These proofs require ElGamal encryption and zero-knowledge proof generation. They **must be generated using the Solana Token-2022 client SDK**, not manually constructed.

---

## Core Endpoints

### POST /transfer-requests

Submit a signed transfer request for processing.

**Request Headers:**

| Header | Required | Description |
|--------|----------|-------------|
| `Content-Type` | Yes | Must be `application/json` |
| `Idempotency-Key` | No | If provided, must match body `nonce` |

**Request Body (Public Transfer):**

```json
{
  "from_address": "7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU",
  "to_address": "DRpbCBMxVnDK7maPM5tGv6MvB3v1sRMC86PZ8okm21hy",
  "transfer_details": {
    "type": "public",
    "amount": 1000000000
  },
  "token_mint": null,
  "signature": "5eykt4UsFv8P8NJdTREpY1vzqKqZKvdpKuc147dw2N9d...",
  "nonce": "019470a4-7e7c-7d3e-8f1a-2b3c4d5e6f7a"
}
```

**Request Body (Confidential Transfer):**

```json
{
  "from_address": "7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU",
  "to_address": "DRpbCBMxVnDK7maPM5tGv6MvB3v1sRMC86PZ8okm21hy",
  "transfer_details": {
    "type": "confidential",
    "equality_proof": "SGVsbG8gRXF1YWxpdHkgUHJvb2Y=",
    "ciphertext_validity_proof": "SGVsbG8gVmFsaWRpdHkgUHJvb2Y=",
    "range_proof": "SGVsbG8gUmFuZ2UgUHJvb2Y=",
    "new_decryptable_available_balance": "SGVsbG8gV29ybGQ="
  },
  "token_mint": "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v",
  "signature": "5eykt4UsFv8P8NJdTREpY1vzqKqZKvdpKuc147dw2N9d...",
  "nonce": "019470a4-7e7c-7d3e-8f1a-2b3c4d5e6f7c"
}
```

**Response (200 OK — new or idempotent return):**

```json
{
  "id": "550e8400-e29b-41d4-a716-446655440000",
  "from_address": "7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU",
  "to_address": "DRpbCBMxVnDK7maPM5tGv6MvB3v1sRMC86PZ8okm21hy",
  "transfer_details": {
    "type": "public",
    "amount": 1000000000
  },
  "token_mint": null,
  "compliance_status": "approved",
  "blockchain_status": "pending_submission",
  "blockchain_signature": null,
  "blockchain_retry_count": 0,
  "blockchain_last_error": null,
  "blockchain_next_retry_at": null,
  "nonce": "019470a4-7e7c-7d3e-8f1a-2b3c4d5e6f7a",
  "created_at": "2026-01-30T10:30:00Z",
  "updated_at": "2026-01-30T10:30:00Z"
}
```

> [!NOTE]
> **Processing Flow:** The request is first persisted with status `received`, then compliance checks run synchronously. If approved, the response shows `blockchain_status: "pending_submission"`. If rejected, the response shows `blockchain_status: "failed"` with an error message.

---

### GET /transfer-requests

List transfers with pagination.

**Query Parameters:**

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `limit` | integer | 20 | Max items (1-100) |
| `cursor` | string | null | ID to start after |

**Response:**

```json
{
  "items": [ ... ],
  "next_cursor": "uuid-of-last-item",
  "has_more": true
}
```

---

### GET /transfer-requests/{id}

Get a single transfer by ID.

**Response Fields:**

| Field | Type | Nullable | Description |
|-------|------|----------|-------------|
| `id` | string | No | UUID of the transfer |
| `from_address` | string | No | Sender wallet (Base58) |
| `to_address` | string | No | Recipient wallet (Base58) |
| `transfer_details` | object | No | Public or Confidential details |
| `token_mint` | string | Yes | SPL token mint (null = SOL) |
| `compliance_status` | enum | No | `pending`, `approved`, `rejected` |
| `blockchain_status` | enum | No | See below |
| `blockchain_signature` | string | **Yes** | On-chain tx signature (null until submitted) |
| `blockchain_retry_count` | integer | No | Number of submission attempts |
| `blockchain_last_error` | string | Yes | Last error message |
| `blockchain_next_retry_at` | datetime | Yes | Next scheduled retry (ISO 8601) |
| `original_tx_signature` | string | Yes | First submission tx signature (Jito double-spend protection) |
| `last_error_type` | enum | No | Classification of last submission error (see below) |
| `blockhash_used` | string | Yes | Blockhash used in last attempt |
| `nonce` | string | Yes | Original request nonce |
| `created_at` | datetime | No | ISO 8601 timestamp |
| `updated_at` | datetime | No | ISO 8601 timestamp |

**Blockchain Status Values:**

| Status | Terminal | Description |
|--------|----------|-------------|
| `received` | No | Initial state. Request persisted immediately before compliance checks. |
| `pending` | No | *(Legacy/Deprecated)* Alias for `received` in older records. |
| `pending_submission` | No | Compliance approved, queued for background worker. |
| `processing` | No | Worker has claimed the task, submission in progress. |
| `submitted` | No | Transaction sent to Solana, awaiting confirmation via webhook or polling. |
| `confirmed` | **Yes** | Transaction finalized on blockchain (finalized commitment). |
| `failed` | **Yes** | Max retries (10) exceeded. May be retryable via `POST /retry`. |
| `expired` | **Yes** | Transaction was not confirmed within the blockhash validity window (~90s). **User must re-sign and submit a new request with a fresh nonce.** |

**Last Error Type Values** (for retry strategy):

| Value | Description |
|-------|--------------|
| `none` | No error or initial state |
| `jito_state_unknown` | Jito bundle state unknown — verify on-chain status before retry |
| `jito_bundle_failed` | Jito bundle rejected; safe to retry with new blockhash |
| `transaction_failed` | Transaction failed on-chain; safe to retry |
| `network_error` | Network/connection error; safe to retry |
| `validation_error` | Validation error; do not retry automatically |

> [!NOTE]
> **Terminal States:** Once a transfer reaches `confirmed`, `failed`, or `expired`, no further automatic processing occurs.
> - `confirmed`: Success - funds transferred.
> - `failed`: Can be manually retried via `POST /transfer-requests/{id}/retry` if the underlying issue is resolved.
> - `expired`: **Cannot be retried.** The original signature is permanently invalid. The user must create and sign a new request.

---

### POST /transfer-requests/{id}/retry

Manually retry a failed submission.

**Eligibility:**

| Status | Retryable | Notes |
|--------|-----------|-------|
| `pending_submission` | ✅ Yes | Re-queues for immediate processing |
| `failed` | ✅ Yes | Resets retry count and re-queues |
| `expired` | ❌ No | Blockhash expired; user must submit a **new request with fresh nonce** |
| `confirmed` | ❌ No | Already successful |
| `rejected` | ❌ No | Compliance rejected (unless blocklist entry removed) |

> [!WARNING]
> **Expired transfers cannot be retried.** The original transaction signature was built with an expired blockhash and can never be confirmed on Solana. The user must create a new transfer request with a fresh nonce and signature.

---

## Admin Endpoints

> [!CAUTION]
> These endpoints have **no application-level authentication**. Restrict access at the network level.

### POST /admin/blocklist

Add an address to the internal blocklist.

**Request:**

```json
{
  "address": "SuspiciousWallet123...",
  "reason": "Suspected phishing activity"
}
```

**Response (200 OK):**

```json
{
  "success": true,
  "message": "Address <address> added to blocklist"
}
```

**Errors:** `400` if `address` or `reason` is empty; `501` if blocklist is not configured.

---

### GET /admin/blocklist

List all blocklisted addresses.

**Response (200 OK):**

```json
{
  "count": 2,
  "entries": [
    { "address": "...", "reason": "Phishing" },
    { "address": "...", "reason": "Sanctions" }
  ]
}
```

**Errors:** `501` if blocklist is not configured.

---

### DELETE /admin/blocklist/{address}

Remove an address from the blocklist.

**Path parameter:** `address` — Base58 wallet address to remove.

**Response (200 OK):**

```json
{
  "success": true,
  "message": "Address <address> removed from blocklist"
}
```

**Errors:** `404` if the address is not in the blocklist; `501` if blocklist is not configured.

---

## Compliance Endpoints

### POST /risk-check

Pre-flight wallet risk check. Aggregates data from internal blocklist, Range Protocol, and Helius DAS. Results are cached for 1 hour.

**Request:**

```json
{
  "address": "7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU"
}
```

**Response (Analyzed):**

```json
{
  "status": "analyzed",
  "address": "7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU",
  "risk_score": 2,
  "risk_level": "Low risk",
  "reasoning": "3 hops from nearest flagged address",
  "has_sanctioned_assets": false,
  "helius_assets_checked": true,
  "from_cache": false,
  "checked_at": "2026-01-30T10:30:00Z"
}
```

**Response (Blocked):**

```json
{
  "status": "blocked",
  "address": "SuspiciousWallet123...",
  "reason": "Internal Security Alert: Address linked to Phishing Scam"
}
```

---

## Webhook Endpoints

### POST /webhooks/helius

Receives Enhanced Transaction events from Helius.

**Required Header:**

```
Authorization: <HELIUS_WEBHOOK_SECRET>
```

**Payload Format:** Array of `HeliusTransaction` objects with `signature` and `transactionError` fields.

---

### POST /webhooks/quicknode

Receives transaction events from QuickNode Streams/Webhooks.

**Authentication Header (one of):**

| Header | Format | Priority |
|--------|--------|----------|
| `x-qn-signature` | `<QUICKNODE_WEBHOOK_SECRET>` | Checked first |
| `Authorization` | `<QUICKNODE_WEBHOOK_SECRET>` | Fallback if `x-qn-signature` is absent |

> [!NOTE]
> When `QUICKNODE_WEBHOOK_SECRET` is set, missing or mismatched headers are **not** currently rejected; the request is accepted and a warning is logged. Use network-level restrictions if strict verification is required.
>
> QuickNode Streams typically send `x-qn-signature`, but some configurations may use `Authorization`. The relayer accepts **either** header for flexibility.

**Payload Format:** Flexible JSON (single event or array of events). The handler extracts `signature` from various nested structures.

> [!NOTE]
> Network fees, priority fees, and compute unit limits are **automatically calculated** by the relayer based on the detected RPC provider (Helius, QuickNode, or standard).

---

## Health Endpoints

| Method | Path | Description |
|--------|------|-------------|
| `GET` | `/health` | Detailed health (database, blockchain). Returns JSON: `status`, `database`, `blockchain`, `timestamp`, `version`. |
| `GET` | `/health/live` | Kubernetes liveness (always 200, no body) |
| `GET` | `/health/ready` | Kubernetes readiness (200 if healthy/degraded, 503 if unhealthy) |

**GET /health response example:**

```json
{
  "status": "healthy",
  "database": "healthy",
  "blockchain": "healthy",
  "timestamp": "2026-01-30T10:30:00Z",
  "version": "0.3.0"
}
```

`status` values: `healthy`, `degraded`, `unhealthy`.

---

## Signing Message Format

### Message Construction

The message to sign is a **UTF-8 encoded string** with the following format:

```
{from_address}:{to_address}:{amount_or_confidential}:{mint_or_SOL}:{nonce}
```

| Component | Value |
|-----------|-------|
| `from_address` | Sender wallet (Base58) |
| `to_address` | Recipient wallet (Base58) |
| `amount_or_confidential` | Numeric amount (e.g., `1000000000`) OR literal `confidential` |
| `mint_or_SOL` | Token mint address (Base58) OR literal `SOL` |
| `nonce` | The unique nonce value |

> [!IMPORTANT]
> **Critical Requirements:**
> - Encode the message as **UTF-8 bytes** (no BOM, no trailing newline/whitespace)
> - Use **exactly** the format above with colons as separators
> - For public transfers, `amount` is the raw `u64` value as a string
> - For confidential transfers, use the literal string `confidential`

### Signature Generation

1. Construct the message string:
   ```
   7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU:DRpbCBMxVnDK7maPM5tGv6MvB3v1sRMC86PZ8okm21hy:1000000000:SOL:019470a4-7e7c-7d3e-8f1a-2b3c4d5e6f7a
   ```

2. Convert to UTF-8 bytes (no BOM):
   ```
   [55, 120, 75, 88, 116, 103, 50, 67, 87, 56, 55, 100, ...]
   ```

3. Sign using **Ed25519** with the sender's private key (standard Solana keypair).

4. Encode the 64-byte signature as **Base58**:
   ```
   5eykt4UsFv8P8NJdTREpY1vzqKqZKvdpKuc147dw2N9d5eykt4UsFv8P8NJdTREpY1vzqKqZKvdpKuc147dw2N9d
   ```

### Example Messages

**Public SOL Transfer (1 SOL):**
```
7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU:RecipientPubkey:1000000000:SOL:019470a4-7e7c-7d3e-8f1a-2b3c4d5e6f7a
```

**SPL Token Transfer (1000 USDC):**
```
7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU:RecipientPubkey:1000000000:EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v:019470a4-7e7c-7d3e-8f1a-2b3c4d5e6f7b
```

**Confidential Transfer:**
```
7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU:RecipientPubkey:confidential:TokenMint:019470a4-7e7c-7d3e-8f1a-2b3c4d5e6f7c
```

---

## Request Uniqueness (Nonce & Idempotency)

### Nonce Requirements

| Requirement | Details |
|-------------|---------|
| **Required** | Yes, in request body. Empty nonce returns `400 Bad Request`. |
| **Format** | **32–64 characters**, **alphanumeric with optional hyphens** (e.g. UUID). Other characters are rejected. |
| **Recommended** | UUID v4 or UUID v7 (time-ordered) |
| **Uniqueness** | Per `(from_address, nonce)` pair |
| **Server Tracking** | Nonces are stored in database and checked for duplicates |

### Idempotency-Key Header

| Behavior | Description |
|----------|-------------|
| **Optional** | Recommended but not required |
| **Must match nonce** | If provided, must exactly equal body `nonce` |
| **Duplicate handling** | Returns existing transfer (200 OK) |

---

## Response Codes

| Code | Meaning |
|------|---------|
| `200` | Success (or idempotent duplicate) |
| `400` | Validation error (invalid signature, missing fields, wrong types) |
| `401` | Authentication failed (webhook secret mismatch; Helius only) |
| `402` | Payment required (insufficient funds for transaction) |
| `403` | Authorization denied (signature verification failed) |
| `404` | Resource not found |
| `409` | Conflict (duplicate record, e.g. nonce already used) |
| `429` | Rate limit exceeded |
| `500` | Internal server error |
| `501` | Feature not configured (e.g., risk service) |
| `503` | Service unavailable (database/blockchain down) |
| `504` | Gateway timeout (upstream timeout) |

---

## Rate Limiting

Rate limiting is **disabled by default**. Set `ENABLE_RATE_LIMITING=true` to enable.

When enabled:

| Setting | Default | Description |
|---------|---------|-------------|
| `RATE_LIMIT_RPS` | 10 | Requests per second |
| `RATE_LIMIT_BURST` | 20 | Burst size |

**Response Headers:**

- On success: `X-RateLimit-Limit` is set to the configured RPS (e.g. `10`). `X-RateLimit-Remaining` is not set on success.
- On 429: `X-RateLimit-Limit`, `X-RateLimit-Remaining: 0`, and `Retry-After` (seconds until reset) are included.

Example when rate limited:

```
X-RateLimit-Limit: 10
X-RateLimit-Remaining: 0
Retry-After: 1
```

---

## Interactive Documentation

| Resource | URL |
|----------|-----|
| **Swagger UI** | `http://localhost:3000/swagger-ui` |
| **OpenAPI Spec** | `http://localhost:3000/api-docs/openapi.json` |

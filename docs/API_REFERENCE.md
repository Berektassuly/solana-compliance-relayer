# API Reference

This document provides the complete API reference for the Solana Compliance Relayer, including endpoints, signing format, and example requests.

---

## Table of Contents

- [Core Endpoints](#core-endpoints)
- [Admin Endpoints](#admin-endpoints)
- [Compliance Endpoints](#compliance-endpoints)
- [Health Endpoints](#health-endpoints)
- [Signing Message Format](#signing-message-format)
- [Request Uniqueness (Nonce & Idempotency)](#request-uniqueness-nonce--idempotency)
- [Example Requests](#example-requests)
- [Interactive Documentation](#interactive-documentation)

---

## Core Endpoints

| Method | Path | Description |
|--------|------|-------------|
| `POST` | `/transfer-requests` | Submit a signed transfer request |
| `GET` | `/transfer-requests` | List transfers (paginated) |
| `GET` | `/transfer-requests/{id}` | Get transfer by ID |
| `POST` | `/transfer-requests/{id}/retry` | Retry failed submission |
| `POST` | `/webhooks/helius` | Helius webhook receiver |
| `POST` | `/webhooks/quicknode` | QuickNode Streams webhook receiver |

---

## Admin Endpoints

| Method | Path | Description |
|--------|------|-------------|
| `POST` | `/admin/blocklist` | Add address to internal blocklist |
| `GET` | `/admin/blocklist` | List all blocklisted addresses |
| `DELETE` | `/admin/blocklist/{address}` | Remove address from blocklist |

### Add to Blocklist

**Request:**

```bash
POST /admin/blocklist
Content-Type: application/json

{
  "address": "SuspiciousWallet123...",
  "reason": "Suspected phishing activity"
}
```

**Response:**

```json
{
  "success": true,
  "message": "Address added to blocklist"
}
```

---

## Compliance Endpoints

| Method | Path | Description |
|--------|------|-------------|
| `POST` | `/risk-check` | Pre-flight wallet risk check |

### Pre-Flight Risk Check

Aggregates blocklist, Range Protocol, and Helius DAS data. Results are cached for 1 hour.

**Request:**

```bash
POST /risk-check
Content-Type: application/json

{
  "address": "<wallet_pubkey>"
}
```

**Response:**

```json
{
  "address": "7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU",
  "risk_score": 3,
  "risk_level": "LOW",
  "blocked": false,
  "reasons": [],
  "cached": false,
  "checked_at": "2026-01-30T12:00:00Z"
}
```

---

## Health Endpoints

| Method | Path | Description |
|--------|------|-------------|
| `GET` | `/health` | Detailed health check (database, RPC, etc.) |
| `GET` | `/health/live` | Kubernetes liveness probe (always returns 200) |
| `GET` | `/health/ready` | Kubernetes readiness probe (checks dependencies) |

---

## Signing Message Format

> **Critical:** Using the wrong format will cause signature verification to fail.

| Version | Format |
|---------|--------|
| **Old (deprecated)** | `{from}:{to}:{amount}:{mint}` |
| **Current (required)** | `{from}:{to}:{amount}:{mint}:{nonce}` |

### Format Details

| Field | Description |
|-------|-------------|
| `from` | Sender wallet public key (Base58) |
| `to` | Recipient wallet public key (Base58) |
| `amount` | For **public** transfers: numeric amount (e.g., `1000000000`). For **confidential**: literal string `confidential` |
| `mint` | For **SOL**: literal string `SOL`. For SPL tokens: mint address (Base58) |
| `nonce` | The unique nonce value (must match the JSON body) |

### Example Messages

**Public SOL Transfer (1 SOL):**

```
7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU:RecipientPubkey...:1000000000:SOL:019470a4-7e7c-7d3e-8f1a-2b3c4d5e6f7a
```

**SPL Token Transfer (1000 USDC, 6 decimals):**

```
7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU:RecipientPubkey...:1000000000:EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v:019470a4-7e7c-7d3e-8f1a-2b3c4d5e6f7a
```

**Confidential Transfer:**

```
7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU:RecipientPubkey...:confidential:TokenMintAddress...:019470a4-7e7c-7d3e-8f1a-2b3c4d5e6f7a
```

---

## Request Uniqueness (Nonce & Idempotency)

Every `POST /transfer-requests` request must include a **nonce** for replay protection and optionally an **Idempotency-Key** header for safe retries.

### Nonce Requirements

| Requirement | Details |
|-------------|---------|
| **Required** | Yes, in the request body |
| **Format** | 32–64 characters, alphanumeric with optional hyphens |
| **Recommended** | UUID v4 or UUID v7 (time-ordered) |
| **Must be in signature** | The signed message must include the nonce |

### Idempotency-Key Header

| Behavior | Description |
|----------|-------------|
| **Optional** | Recommended but not required |
| **Must match nonce** | If provided, must equal the body `nonce` |
| **Duplicate handling** | Returns existing transfer (200 OK) instead of creating duplicate |

---

## Example Requests

### Submit Public SOL Transfer

```bash
curl -X POST http://localhost:3000/transfer-requests \
  -H "Content-Type: application/json" \
  -H "Idempotency-Key: 019470a4-7e7c-7d3e-8f1a-2b3c4d5e6f7a" \
  -d '{
    "from_address": "7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU",
    "to_address": "RecipientPubkeyHere...",
    "transfer_details": {
      "type": "public",
      "amount": 1000000000
    },
    "token_mint": null,
    "signature": "BASE58_ED25519_SIGNATURE_OVER_MESSAGE_WITH_NONCE",
    "nonce": "019470a4-7e7c-7d3e-8f1a-2b3c4d5e6f7a"
  }'
```

### Submit SPL Token Transfer

```bash
curl -X POST http://localhost:3000/transfer-requests \
  -H "Content-Type: application/json" \
  -H "Idempotency-Key: 019470a4-7e7c-7d3e-8f1a-2b3c4d5e6f7b" \
  -d '{
    "from_address": "7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU",
    "to_address": "RecipientPubkeyHere...",
    "transfer_details": {
      "type": "public",
      "amount": 1000000000
    },
    "token_mint": "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v",
    "signature": "BASE58_ED25519_SIGNATURE",
    "nonce": "019470a4-7e7c-7d3e-8f1a-2b3c4d5e6f7b"
  }'
```

### Submit Confidential Transfer

```bash
curl -X POST http://localhost:3000/transfer-requests \
  -H "Content-Type: application/json" \
  -H "Idempotency-Key: 019470a4-7e7c-7d3e-8f1a-2b3c4d5e6f7c" \
  -d '{
    "from_address": "7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU",
    "to_address": "RecipientPubkeyHere...",
    "transfer_details": {
      "type": "confidential",
      "equality_proof": "BASE64_ENCODED_PROOF",
      "ciphertext_validity_proof": "BASE64_ENCODED_PROOF",
      "range_proof": "BASE64_ENCODED_PROOF",
      "new_decryptable_available_balance": "BASE64_ENCODED_BALANCE"
    },
    "token_mint": "TokenMintAddress...",
    "signature": "BASE58_ED25519_SIGNATURE",
    "nonce": "019470a4-7e7c-7d3e-8f1a-2b3c4d5e6f7c"
  }'
```

### Get Transfer by ID

```bash
curl http://localhost:3000/transfer-requests/550e8400-e29b-41d4-a716-446655440000
```

### List Transfers (Paginated)

```bash
# Get first page (default limit: 50)
curl http://localhost:3000/transfer-requests

# With pagination
curl "http://localhost:3000/transfer-requests?limit=20&offset=40"
```

### Retry Failed Transfer

```bash
curl -X POST http://localhost:3000/transfer-requests/550e8400-e29b-41d4-a716-446655440000/retry
```

### Pre-Flight Risk Check

```bash
curl -X POST http://localhost:3000/risk-check \
  -H "Content-Type: application/json" \
  -d '{
    "address": "7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU"
  }'
```

---

## Interactive Documentation

The API includes auto-generated OpenAPI documentation:

| Resource | URL |
|----------|-----|
| **Swagger UI** | `http://localhost:3000/swagger-ui` |
| **OpenAPI Spec (JSON)** | `http://localhost:3000/api-docs/openapi.json` |

---

## Response Codes

| Code | Meaning |
|------|---------|
| `200` | Success (or duplicate idempotent request) |
| `400` | Bad request (invalid signature, missing fields, etc.) |
| `404` | Transfer not found |
| `429` | Rate limit exceeded |
| `500` | Internal server error |

---

## Rate Limiting

When rate limiting is enabled (`ENABLE_RATE_LIMITING=true`), the API enforces request limits:

| Setting | Default | Description |
|---------|---------|-------------|
| `RATE_LIMIT_RPS` | 10 | Requests per second |
| `RATE_LIMIT_BURST` | 20 | Burst size |

Rate limit responses include headers:

```
X-RateLimit-Limit: 10
X-RateLimit-Remaining: 5
X-RateLimit-Reset: 1706616000
```

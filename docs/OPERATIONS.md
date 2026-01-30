# Technical Operations Guide

This document provides enterprise-grade operational guidance for the Solana Compliance Relayer. It covers infrastructure configuration, manual testing procedures, database operations, failure recovery, and performance tuning.

> [!IMPORTANT]
> This guide is validated against the current codebase. Configuration values and code references are accurate as of the document revision date.

---

## Table of Contents

1. [Infrastructure Configuration](#1-infrastructure-configuration)
   - [Helius Webhook Authentication](#helius-webhook-authentication)
   - [Range Protocol Integration](#range-protocol-integration)
   - [RPC Provider Auto-Detection](#rpc-provider-auto-detection)
2. [Signing Message Format](#2-signing-message-format)
3. [Manual Testing Playbook](#3-manual-testing-playbook)
4. [Database Operations](#4-database-operations)
   - [Worker Claim Mechanism](#worker-claim-mechanism)
   - [Inspection Queries](#inspection-queries)
5. [Troubleshooting Reference](#5-troubleshooting-reference)
6. [Security Operations](#6-security-operations)
7. [Performance Tuning](#7-performance-tuning)

---

## 1. Infrastructure Configuration

### Helius Webhook Authentication

The Helius webhook handler validates incoming requests by comparing the **raw** `Authorization` header value to the configured `HELIUS_WEBHOOK_SECRET` environment variable. This is an **exact string match** — no Bearer prefix is stripped, and no HMAC or digest is computed.

**Source of Truth**: `src/api/handlers.rs`, `helius_webhook_handler` (lines 281–293)

```rust
if let Some(expected_secret) = &state.helius_webhook_secret {
    let auth_header = headers
        .get("Authorization")
        .and_then(|v| v.to_str().ok())
        .ok_or_else(|| AppError::Authentication("Missing Authorization header".to_string()))?;

    if auth_header != expected_secret {
        return Err(AppError::Authentication(
            "Invalid webhook secret".to_string(),
        ));
    }
}
```

**Configuration Procedure**:

1. Generate a cryptographically random secret (minimum 32 characters):
   ```bash
   openssl rand -base64 32
   ```

2. In the Helius Dashboard webhook configuration, set **Auth Header** to the **literal secret value** (the exact string, not `Bearer <secret>`):
   ```
   your-secret-value-no-bearer-prefix
   ```

3. Set the identical value in your environment:
   ```bash
   HELIUS_WEBHOOK_SECRET=your-secret-value-no-bearer-prefix
   ```

> [!CAUTION]
> **Header must match exactly.** If Helius sends `Bearer <secret>` but your environment variable contains only `<secret>`, authentication will fail. Ensure both the Helius dashboard and `HELIUS_WEBHOOK_SECRET` use the exact same string (with or without `Bearer ` consistently on both sides).

---

### Range Protocol Integration

#### Mock Mode Detection

The compliance provider runs in **mock mode** when `RANGE_API_KEY` is not set or is empty. In mock mode, the provider uses a deterministic address-matching function and does not call the Range Protocol API.

**Source of Truth**: `src/infra/compliance/range.rs`

- Mock mode detection (lines 111–114):
  ```rust
  fn is_mock_mode(&self) -> bool {
      self.api_key.is_none()
  }
  ```

- Mock check logic (lines 116–129):
  ```rust
  fn mock_check(&self, to_address: &str) -> ComplianceStatus {
      if to_address == "hack_the_planet_bad_wallet" {
          return ComplianceStatus::Rejected;
      }
      if to_address.to_lowercase().starts_with("hack") {
          return ComplianceStatus::Rejected;
      }
      ComplianceStatus::Approved
  }
  ```

**Mock Mode Rejection Patterns**:

| Pattern | Example | Result |
|---------|---------|--------|
| Exact match | `hack_the_planet_bad_wallet` | Rejected |
| Prefix match (case-insensitive) | `hackMaliciousWallet`, `HACKer` | Rejected |
| All other addresses | `DRpbCBMxVnDK...` | Approved |

#### Risk Threshold Configuration

The risk threshold is configurable via the **environment variable** `RANGE_RISK_THRESHOLD`. The default is `6` (High Risk).

**Source of Truth**: `src/infra/compliance/range.rs` (line 18)

```rust
pub const DEFAULT_RISK_THRESHOLD: i32 = 6;
```

**Evaluation Logic** (lines 210–243 in `range.rs`):

- Primary: `risk_score >= threshold` → Rejected
- Secondary: Text pattern matching for "high", "severe", "extremely", "critical" (conditional on threshold)
- API errors default to **Rejected** for safety

| Threshold | `RANGE_RISK_THRESHOLD` | Behavior |
|-----------|------------------------|----------|
| Strict | `2` | Reject scores ≥ 2 (Low risk and above) |
| Default | `6` | Reject scores ≥ 6 (High risk and above) |
| Relaxed | `8` | Reject scores ≥ 8 (Extremely high and above) |

---

### RPC Provider Auto-Detection

The system infers the RPC provider from the RPC URL via substring matching and enables provider-specific behavior (e.g. priority fee APIs).

**Source of Truth**: `src/infra/blockchain/strategies.rs`, `RpcProviderType::detect` (lines 40–50)

```rust
pub fn detect(rpc_url: &str) -> Self {
    let url_lower = rpc_url.to_lowercase();

    if url_lower.contains("helius-rpc.com") || url_lower.contains("helius.xyz") {
        RpcProviderType::Helius
    } else if url_lower.contains("quiknode.pro") || url_lower.contains("quicknode.com") {
        RpcProviderType::QuickNode
    } else {
        RpcProviderType::Standard
    }
}
```

**Feature Matrix**:

| Provider | Detection Pattern | Priority Fee API | DAS Compliance | Webhooks |
|----------|-------------------|------------------|----------------|----------|
| Helius | `helius-rpc.com`, `helius.xyz` | `getPriorityFeeEstimate` | Yes | Yes |
| QuickNode | `quiknode.pro`, `quicknode.com` | `qn_estimatePriorityFees` | No | Yes |
| Standard | (fallback) | Static 100 µ-lamports | No | No |

---

## 2. Signing Message Format

> [!IMPORTANT]
> **Replay protection requires a nonce.** The signing message **MUST** include a unique nonce. Clients that omit the nonce will fail signature verification.

**Message format** (five colon-separated fields):

```
{from_address}:{to_address}:{amount|confidential}:{token_mint|SOL}:{nonce}
```

**Source of Truth**: `src/domain/types.rs`, `SubmitTransferRequest::create_signing_message` (lines 436–451)

```rust
/// Format: "{from_address}:{to_address}:{amount|confidential}:{token_mint|SOL}:{nonce}"
pub fn create_signing_message(&self) -> Vec<u8> {
    let amount_part = match &self.transfer_details {
        TransferType::Public { amount } => amount.to_string(),
        TransferType::Confidential { .. } => "confidential".to_string(),
    };
    let mint_part = self.token_mint.as_deref().unwrap_or("SOL");
    format!(
        "{}:{}:{}:{}:{}",
        self.from_address, self.to_address, amount_part, mint_part, self.nonce
    )
    .into_bytes()
}
```

The **nonce is included** as the fifth field in the format string above.

**Example messages**:

| Transfer Type | Message |
|---------------|---------|
| SOL Public | `7xKX...AsU:DRpb...1hy:1000000000:SOL:019470a4-7e7c-7d3e-8f1a-2b3c4d5e6f7a` |
| SPL Token | `7xKX...AsU:DRpb...1hy:1000000:EPjF...t1v:019470a4-7e7c-7d3e-8f1a-2b3c4d5e6f7a` |
| Confidential | `7xKX...AsU:DRpb...1hy:confidential:MINT:019470a4-7e7c-7d3e-8f1a-2b3c4d5e6f7a` |

**Nonce requirements** (from `src/domain/types.rs`, `SubmitTransferRequest` validation):

- Required; cannot be empty
- Length: 32–64 characters
- Characters: alphanumeric and hyphens only (UUID format recommended)

> [!WARNING]
> WASM and other clients must build the signing message using this **exact** format. A missing nonce, extra whitespace, or different field order will cause `Signature verification failed`.

---

## 3. Manual Testing Playbook

### Generate Signed Test Requests

Use the built-in CLI to generate valid signed requests with correct nonces:

```bash
# Public transfer
cargo run --bin generate_transfer_request

# Confidential transfer
cargo run --bin generate_transfer_request -- --confidential
```

### Public Transfer Test

```bash
curl -X POST 'http://localhost:3000/transfer-requests' \
  -H 'Content-Type: application/json' \
  -d '{
    "from_address": "YOUR_WALLET_PUBKEY",
    "to_address": "RECIPIENT_PUBKEY",
    "transfer_details": {
      "type": "public",
      "amount": 1000000000
    },
    "nonce": "019470a4-7e7c-7d3e-8f1a-2b3c4d5e6f7a",
    "signature": "VALID_BASE58_SIGNATURE"
  }'
```

**Expected response** (structure):

```json
{
  "id": "550e8400-e29b-41d4-a716-446655440000",
  "compliance_status": "approved",
  "blockchain_status": "pending_submission"
}
```

### Compliance Rejection Test (Mock Mode)

With `RANGE_API_KEY` unset, sending a request whose `to_address` matches mock rejection rules should return `compliance_status: "rejected"`:

```bash
curl -X POST 'http://localhost:3000/transfer-requests' \
  -H 'Content-Type: application/json' \
  -d '{
    "from_address": "ValidSenderAddress",
    "to_address": "hackMaliciousAddress123",
    "transfer_details": {"type": "public", "amount": 1000000000},
    "nonce": "019470a4-7e7c-7d3e-8f1a-2b3c4d5e6f7b",
    "signature": "VALID_SIGNATURE_FOR_THIS_MESSAGE"
  }'
```

### Pre-Flight Risk Check

```bash
curl -X POST 'http://localhost:3000/risk-check' \
  -H 'Content-Type: application/json' \
  -d '{"address": "HvwC9QSAzwEXkUkwqNNGhfNHoVqXJYfPvPZfQvJmHWcF"}'
```

---

## 4. Database Operations

### Worker Claim Mechanism

The background worker claims pending transfer requests using PostgreSQL row locking so that multiple worker replicas do not double-process the same rows.

**Source of Truth**: `src/infra/database/postgres.rs`, `get_pending_blockchain_requests` (lines 349–368)

The implementation uses an `UPDATE ... WHERE id IN (SELECT ... FOR UPDATE SKIP LOCKED) RETURNING *` pattern:

```sql
UPDATE transfer_requests
SET blockchain_status = 'processing',
    updated_at = NOW()
WHERE id IN (
    SELECT id FROM transfer_requests
    WHERE blockchain_status = 'pending_submission'
      AND compliance_status = 'approved'
      AND (blockchain_next_retry_at IS NULL OR blockchain_next_retry_at <= $1)
      AND blockchain_retry_count < 10
    ORDER BY blockchain_next_retry_at ASC NULLS FIRST, created_at ASC
    LIMIT $2
    FOR UPDATE SKIP LOCKED
)
RETURNING *;
```

`FOR UPDATE SKIP LOCKED` ensures that only one worker instance can claim each row; others skip locked rows and process different rows.

**Worker configuration** (code-only; not environment-configurable):

| Parameter | Default | Location |
|-----------|---------|----------|
| `poll_interval` | 10 seconds | `src/app/worker.rs`, `WorkerConfig::default()` (line 30) |
| `batch_size` | 10 | `src/app/worker.rs`, `WorkerConfig::default()` (line 31) |
| Max retry count | 10 | Query filter `blockchain_retry_count < 10` |

---

### Inspection Queries

The following queries use columns that exist on `transfer_requests` in the current schema (see migrations and `src/infra/database/postgres.rs`).

#### Transaction Status Distribution

```sql
SELECT
    blockchain_status,
    compliance_status,
    COUNT(*) AS count
FROM transfer_requests
GROUP BY blockchain_status, compliance_status
ORDER BY count DESC;
```

#### Stuck Processing Transactions

Identifies rows left in `processing` for longer than 5 minutes (e.g. after a worker crash):

```sql
SELECT id, from_address, blockchain_status, updated_at,
       NOW() - updated_at AS stuck_duration
FROM transfer_requests
WHERE blockchain_status = 'processing'
  AND updated_at < NOW() - INTERVAL '5 minutes'
ORDER BY updated_at ASC;
```

#### Reset Stuck Transactions

> [!CAUTION]
> Run only after confirming the worker has crashed or is no longer processing these rows. Resetting active work can cause duplicate submissions or inconsistent state.

```sql
UPDATE transfer_requests
SET blockchain_status = 'pending_submission',
    updated_at = NOW()
WHERE blockchain_status = 'processing'
  AND updated_at < NOW() - INTERVAL '10 minutes';
```

#### High Retry Count Investigation

```sql
SELECT id, blockchain_retry_count, blockchain_last_error,
       blockchain_next_retry_at, updated_at
FROM transfer_requests
WHERE blockchain_retry_count > 3
  AND blockchain_status != 'confirmed'
ORDER BY blockchain_retry_count DESC
LIMIT 20;
```

---

## 5. Troubleshooting Reference

| Symptom | Root Cause | Resolution |
|---------|------------|------------|
| `401 Unauthorized` on webhook | `HELIUS_WEBHOOK_SECRET` mismatch | Use exact same string (no Bearer prefix unless both sides use it). Check Helius dashboard and env. |
| `Signature verification failed` | Message format mismatch | Ensure nonce is included. Format: `{from}:{to}:{amount}:{mint}:{nonce}`. |
| Transactions stuck in `processing` | Worker crashed mid-cycle | After 10+ minutes, reset via SQL above. Check worker logs for panics. |
| `pool timed out` | Connection pool exhaustion | Increase `max_connections` in `PostgresConfig`. Requires code change and redeploy (see [Performance Tuning](#7-performance-tuning)). |
| Compliance always `rejected` | Range API unreachable or error | API errors default to rejection. Verify `RANGE_API_KEY` and network. |
| Webhook received but not processed | Signature not found in DB | Ensure relayer wallet pubkey is in Helius webhook "Account Addresses". |
| Worker not processing | Disabled or crashed | Set `ENABLE_BACKGROUND_WORKER=true`. Check logs for errors. |

---

## 6. Security Operations

### Rotating the Issuer Private Key

> [!CAUTION]
> **Key rotation is a critical change.** Coordinate wallet funding, webhook configuration, and deployment. Performing steps out of order can cause failed transactions or missed webhook updates.

**Procedure**:

1. **Generate new keypair**:
   ```bash
   solana-keygen new --outfile new-relayer-keypair.json
   ```

2. **Fund the new wallet**:
   ```bash
   solana transfer NEW_PUBKEY 1.0 --from OLD_KEYPAIR
   ```

3. **Update Helius webhook**: Add the new public key to "Account Addresses". Keep the old key temporarily so in-flight transactions can still be matched.

4. **Export as Base58**: Convert the 64-byte keypair array to Base58 for `ISSUER_PRIVATE_KEY`.

5. **Deploy with new key**: Update the `ISSUER_PRIVATE_KEY` environment variable in your deployment platform (e.g. Railway).

6. **Verify**: Confirm that new transactions are submitted and confirmed using the new key.

7. **Remove old key**: After verification, remove the old pubkey from the Helius webhook "Account Addresses".

---

## 7. Performance Tuning

### PostgreSQL Connection Pool

Pool limits are defined in code only. There is no environment variable for pool size; changing it requires editing the source and redeploying.

**Source of Truth**: `src/infra/database/postgres.rs`, `PostgresConfig::default()` (lines 25–35)

```rust
impl Default for PostgresConfig {
    fn default() -> Self {
        Self {
            max_connections: 10,
            min_connections: 2,
            acquire_timeout: Duration::from_secs(3),
            idle_timeout: Duration::from_secs(600),
            max_lifetime: Duration::from_secs(1800),
        }
    }
}
```

The application uses `PostgresConfig::default()` at startup (see `src/main.rs`). To change pool size, instantiate a custom `PostgresConfig` (e.g. with a higher `max_connections`) where the client is constructed and redeploy.

**Railway tier guidance**:

| Tier | `max_connections` | Notes |
|------|--------------------|--------|
| Starter | 5–10 | Default is usually sufficient |
| Pro | 20–30 | Edit source, redeploy |
| Scale | 50–100+ | Edit source, redeploy; ensure DB supports the limit |

### Rate Limiting

| Variable | Default | Description |
|----------|---------|-------------|
| `ENABLE_RATE_LIMITING` | `false` | Enable Governor middleware |
| `RATE_LIMIT_RPS` | `10` | Requests per second |
| `RATE_LIMIT_BURST` | `20` | Burst allowance |

### Worker Parameters

| Variable | Default | Description |
|----------|---------|-------------|
| `ENABLE_BACKGROUND_WORKER` | `true` | Enable/disable background worker |
| `ENABLE_PRIVACY_CHECKS` | `true` | Anonymity set checks for confidential transfers |

> [!NOTE]
> `poll_interval` and `batch_size` are not configurable via environment. Adjust `WorkerConfig::default()` in `src/app/worker.rs` and redeploy to tune.

---

## Appendix: Environment Variables

| Variable | Required | Default | Description |
|----------|----------|---------|-------------|
| `DATABASE_URL` | Yes | — | PostgreSQL connection string |
| `SOLANA_RPC_URL` | No | `https://api.devnet.solana.com` | Solana RPC endpoint |
| `ISSUER_PRIVATE_KEY` | Yes | — | Base58-encoded relayer wallet key |
| `RANGE_API_KEY` | No | — | Range Protocol API key (mock mode if absent) |
| `RANGE_API_URL` | No | `https://api.range.org/v1` | Range API base URL |
| `RANGE_RISK_THRESHOLD` | No | `6` | Risk threshold (1–10) |
| `HELIUS_WEBHOOK_SECRET` | No | — | Exact Authorization header value for Helius webhooks |
| `QUICKNODE_WEBHOOK_SECRET` | No | — | QuickNode webhook secret (x-qn-signature or Authorization) |
| `ENABLE_RATE_LIMITING` | No | `false` | Governor middleware toggle |
| `RATE_LIMIT_RPS` | No | `10` | Requests per second |
| `RATE_LIMIT_BURST` | No | `20` | Burst allowance |
| `ENABLE_BACKGROUND_WORKER` | No | `true` | Background worker toggle |
| `ENABLE_PRIVACY_CHECKS` | No | `true` | Privacy health checks for confidential transfers |
| `HOST` | No | `0.0.0.0` | Bind address |
| `PORT` | No | `3000` | Bind port |
| `CORS_ALLOWED_ORIGINS` | No | See `.env.example` | Comma-separated CORS origins |

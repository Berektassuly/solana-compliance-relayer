# Solana Compliance Relayer: Technical Operations Guide

This document provides a comprehensive technical manual for developers and operators who need to maintain, test, and troubleshoot the Solana Compliance Relayer beyond basic setup. It covers infrastructure configuration, manual testing procedures, database operations, and failure recovery.

---

## Table of Contents

1. [Advanced Infrastructure Setup](#1-advanced-infrastructure-setup)
   - [Helius Webhook Configuration](#helius-webhook-configuration)
   - [Range Protocol Integration](#range-protocol-integration)
   - [RPC Provider Auto-Detection](#rpc-provider-auto-detection)
2. [WASM Development and Compilation](#2-wasm-development-and-compilation)
   - [Toolchain Requirements](#toolchain-requirements)
   - [Build and Rebuild Process](#build-and-rebuild-process)
   - [Troubleshooting WASM Issues](#troubleshooting-wasm-issues)
3. [Manual Testing Playbook](#3-manual-testing-playbook)
   - [Public Transfer Tests](#public-transfer-tests)
   - [Confidential Transfer Tests](#confidential-transfer-tests)
   - [Retry Logic Tests](#retry-logic-tests)
   - [Compliance Failure Simulation](#compliance-failure-simulation)
   - [Pre-Flight Risk Check Tests](#pre-flight-risk-check-tests)
4. [Transaction Lifecycle and Database Operations](#4-transaction-lifecycle-and-database-operations)
   - [State Machine Deep Dive](#state-machine-deep-dive)
   - [Database Inspection Queries](#database-inspection-queries)
   - [Worker Claim Mechanism](#worker-claim-mechanism)
5. [Comprehensive Troubleshooting](#5-comprehensive-troubleshooting)
6. [Security and Performance Tuning](#6-security-and-performance-tuning)



---

## 1. Advanced Infrastructure Setup

### Helius Webhook Configuration

The Helius webhook is responsible for notifying the backend when transactions are finalized on-chain. This moves transactions from `submitted` to `confirmed` status.

#### Step 1: Navigate to Helius Dashboard

1. Go to [https://dev.helius.xyz/](https://dev.helius.xyz/)
2. Select your project or create a new one
3. Navigate to **Webhooks** in the sidebar

#### Step 2: Create the Webhook

| Field | Value |
|-------|-------|
| **Webhook URL** | `https://your-backend.railway.app/webhooks/helius` |
| **Network** | `mainnet-beta` (or `devnet` for testing) |
| **Webhook Type** | `Enhanced` (critical - provides parsed transaction data) |
| **Transaction Types** | `TRANSFER`, `TOKEN_TRANSFER` (at minimum) |
| **Account Addresses** | Your relayer wallet public key |

> **CAUTION**: You must select **Enhanced** transaction type. The standard webhook format does not include the `signature` field in the expected location, causing webhook processing to fail silently.

#### Step 3: Configure the Authorization Header

The backend compares the **raw value** of the `Authorization` header to `HELIUS_WEBHOOK_SECRET` (exact string match, no `Bearer ` prefix).

In the Helius dashboard, set **Auth Header** to the literal secret value, e.g.:

```
your-secret-value-here
```

Then set the **same** value in your environment:

```bash
HELIUS_WEBHOOK_SECRET=your-secret-value-here
```

The backend uses `headers.get("Authorization")` and checks `auth_header == expected_secret`. Ensure Helius sends the header value as exactly this string.

#### Step 4: Verify the Handshake

The backend validates incoming webhooks in `handlers.rs`:

```rust
// From src/api/handlers.rs
if let Some(expected_secret) = &state.helius_webhook_secret {
    let auth_header = headers
        .get("Authorization")
        .and_then(|v| v.to_str().ok())
        .ok_or_else(|| AppError::Authentication("Missing Authorization header".to_string()))?;

    if auth_header != expected_secret {
        return Err(AppError::Authentication("Invalid webhook secret".to_string()));
    }
}
```

#### Step 5: Monitor Webhook Delivery

Check your application logs for messages like:

```
Helius webhook processed received=1 processed=1
```

(Exact format depends on your `tracing` configuration; look for `received` and `processed`.)

If you see `received=1 processed=0`, the signature was not found in the database. Verify:
- The relayer wallet public key is in the webhook's Account Addresses
- Transactions are being submitted with that wallet as the fee payer

---

### Range Protocol Integration

#### Mock Mode vs Production Mode

The system operates in **Mock Mode** when `RANGE_API_KEY` is not set or empty. This is useful for development but must be disabled in production.

**Mock Mode Behavior** (from `src/infra/compliance/range.rs`):

Mock mode is active when `RANGE_API_KEY` is **not set or empty**. The provider uses `mock_check`:

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

**Production Mode Activation**:

```env
RANGE_API_KEY=your-range-protocol-api-key
RANGE_API_URL=https://api.range.org/v1  # Optional, uses default if not set
RANGE_RISK_THRESHOLD=6  # Optional, 1-10 (default: 6 = High Risk)
```

#### Risk Score Evaluation

The system evaluates risk responses using a **configurable threshold** (default: 6):

```rust
fn evaluate_risk(&self, response: &RiskResponse) -> ComplianceStatus {
    // Primary check: numeric risk score against configured threshold
    let exceeds_threshold = response.risk_score >= self.risk_threshold;
    
    // Text-based checks are conditional on the threshold level
    let text_indicates_risk = (self.risk_threshold <= 6 && risk_level_lower.contains("high"))
        || (self.risk_threshold <= 8
            && (risk_level_lower.contains("severe") || risk_level_lower.contains("extremely")))
        || risk_level_lower.contains("critical");
    
    if exceeds_threshold || text_indicates_risk {
        ComplianceStatus::Rejected
    } else {
        ComplianceStatus::Approved
    }
}
```

| Risk Score | Risk Level | Default (threshold=6) | Strict (threshold=2) | Relaxed (threshold=8) |
|------------|------------|----------------------|---------------------|----------------------|
| 1 | Very Low | Approved | Approved | Approved |
| 2-3 | Low | Approved | Rejected | Approved |
| 4-5 | Medium | Approved | Rejected | Approved |
| 6-7 | High | Rejected | Rejected | Approved |
| 8-9 | Extremely High | Rejected | Rejected | Rejected |
| 10 | Critical | Rejected | Rejected | Rejected |

> **NOTE**: On API errors, the system defaults to **Rejected** for safety. This prevents potentially sanctioned transactions from being processed during service degradation.

---

### RPC Provider Auto-Detection

The relayer implements a Strategy Pattern that auto-detects your RPC provider and activates premium features.

**Detection Logic** (from `src/infra/blockchain/strategies.rs`):

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

| Provider | Priority Fees | DAS Compliance | Webhooks | Privacy Health |
|----------|---------------|----------------|----------|----------------|
| Helius | `getPriorityFeeEstimate` | Yes | Yes | No |
| QuickNode | `qn_estimatePriorityFees` | No | No | Yes |
| Standard | Static (100 micro-lamports) | No | No | No |

---

## 2. WASM Development and Compilation

### Toolchain Requirements

Install the following tools:

```bash
# Install wasm-pack
curl https://rustwasm.github.io/wasm-pack/installer/init.sh -sSf | sh

# Add the WASM target to Rust
rustup target add wasm32-unknown-unknown

# Verify installation
wasm-pack --version
rustup target list --installed | grep wasm32
```

### Build and Rebuild Process

The WASM module resides in the frontend repository. To rebuild:

```bash
cd solana-compliance-relayer-frontend/wasm-signer

# Build for web target
wasm-pack build --target web --out-dir ../src/lib/wasm-pkg

# The output directory will contain:
# - wasm_signer_bg.wasm    (compiled WebAssembly binary)
# - wasm_signer.js         (JavaScript glue code)
# - wasm_signer.d.ts       (TypeScript declarations)
```

**Ensure Next.js Picks Up Changes**:

1. Stop the Next.js development server
2. Delete `.next` cache directory:
   ```bash
   rm -rf .next
   ```
3. Restart the development server:
   ```bash
   pnpm run dev
   ```

### Troubleshooting WASM Issues

#### Signature Mismatch Between Browser and Backend

The signing message format must be identical on both sides:

**Message Format**:
```
{from_address}:{to_address}:{amount|confidential}:{token_mint|SOL}
```

**Example**:
```
7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU:DRpbCBMxVnDK7maPM5tGv6MvB3v1sRMC86PZ8okm21hy:1000000000:SOL
```

**Common Causes of Mismatch**:

| Issue | Symptom | Solution |
|-------|---------|----------|
| Whitespace differences | `Authorization error: Signature verification failed` | Ensure no extra spaces or newlines in message |
| Amount encoding | Signature invalid | Use raw lamports (u64), not formatted SOL |
| Token mint casing | Signature invalid | Use exact Base58 address, case-sensitive |

**Debug Technique**:

Add logging to both WASM and backend to compare the exact bytes being signed. The backend uses `SubmitTransferRequest::create_signing_message()` in `src/domain/types.rs`, which returns `Vec<u8>`. To log the string:

```rust
// Backend: src/domain/types.rs — temporary debug addition
pub fn create_signing_message(&self) -> Vec<u8> {
    let amount_part = match &self.transfer_details {
        TransferType::Public { amount } => amount.to_string(),
        TransferType::Confidential { .. } => "confidential".to_string(),
    };
    let mint_part = self.token_mint.as_deref().unwrap_or("SOL");
    let msg = format!("{}:{}:{}:{}", self.from_address, self.to_address, amount_part, mint_part);
    tracing::debug!(message = %msg, "Signing message constructed");
    msg.into_bytes()
}
```

Ensure the format is exactly `{from}:{to}:{amount|confidential}:{token_mint|SOL}` with no extra spaces or newlines.

---

## 3. Manual Testing Playbook

### Public Transfer Tests

#### Generate a Valid Signed Request

Use the built-in CLI tool:

```bash
cargo run --bin generate_transfer_request
```

This outputs a ready-to-use curl command with a valid Ed25519 signature.

#### Submit Directly

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
    "signature": "VALID_BASE58_SIGNATURE"
  }'
```

**Expected Response** (success):

```json
{
  "id": "550e8400-e29b-41d4-a716-446655440000",
  "compliance_status": "approved",
  "blockchain_status": "pending_submission"
}
```

---

### Confidential Transfer Tests

Generate a confidential transfer with real ZK proofs:

```bash
cargo run --bin generate_transfer_request -- --confidential
```

The binary supports only `--confidential`; there is no `--help` flag. Other args are ignored.

This generates:
- ElGamal keypairs for source and destination
- AES encryption key
- Equality proof (proves balance correctness)
- Ciphertext validity proof (proves encryption correctness)
- Range proof (proves non-negative amounts)

**Sample Request Structure**:

```json
{
  "from_address": "...",
  "to_address": "...",
  "transfer_details": {
    "type": "confidential",
    "new_decryptable_available_balance": "BASE64_AES_CIPHERTEXT",
    "equality_proof": "BASE64_PROOF_DATA",
    "ciphertext_validity_proof": "BASE64_PROOF_DATA",
    "range_proof": "BASE64_PROOF_DATA"
  },
  "token_mint": "TOKEN_MINT_ADDRESS",
  "signature": "..."
}
```

---

### Retry Logic Tests

#### Find a Failed Transaction

```bash
curl 'http://localhost:3000/transfer-requests?limit=10' | \
  jq '.items[] | select(.blockchain_status == "failed")'
```

#### Trigger Manual Retry

```bash
curl -X POST 'http://localhost:3000/transfer-requests/{id}/retry'
```

**Expected Behavior**:
- If eligible (status is `failed` or `pending_submission`): Returns updated request
- If not eligible: Returns `400 Bad Request`

---

### Compliance Failure Simulation

In **Mock Mode**, addresses starting with "hack" are automatically rejected:

```bash
curl -X POST 'http://localhost:3000/transfer-requests' \
  -H 'Content-Type: application/json' \
  -d '{
    "from_address": "ValidSenderAddress",
    "to_address": "hackMaliciousAddress123",
    "transfer_details": {"type": "public", "amount": 1000000000},
    "signature": "VALID_SIGNATURE_FOR_THIS_MESSAGE"
  }'
```

**Expected Response**:

```json
{
  "compliance_status": "rejected",
  "blockchain_status": "failed"
}
```

Rejected requests are persisted with `blockchain_status: failed` and a `blockchain_last_error` reason (e.g. `Blocklist: ...` or `Range Protocol: ...`).

Verify persistence:

```sql
SELECT id, to_address, compliance_status, blockchain_status 
FROM transfer_requests 
WHERE compliance_status = 'rejected' 
ORDER BY created_at DESC LIMIT 5;
```

---

### Pre-Flight Risk Check Tests

The `/risk-check` endpoint performs pre-flight compliance screening without initiating a transaction.

#### Check a Clean Address

```bash
curl -X POST 'http://localhost:3000/risk-check' \
  -H 'Content-Type: application/json' \
  -d '{
    "address": "HvwC9QSAzwEXkUkwqNNGhfNHoVqXJYfPvPZfQvJmHWcF"
  }'
```

**Expected Response (Analyzed)**:

```json
{
  "status": "analyzed",
  "address": "HvwC9QSAzwEXkUkwqNNGhfNHoVqXJYfPvPZfQvJmHWcF",
  "risk_score": 2,
  "risk_level": "Low risk",
  "reasoning": "3 hops from nearest flagged address",
  "has_sanctioned_assets": false,
  "helius_assets_checked": true,
  "from_cache": false,
  "checked_at": "2026-01-23T12:00:00Z"
}
```

#### Check a Blocked Address

```bash
curl -X POST 'http://localhost:3000/risk-check' \
  -H 'Content-Type: application/json' \
  -d '{
    "address": "4oS78GPe66RqBduuAeiMFANf27FpmgXNwokZ3ocN4z1B"
  }'
```

**Expected Response (Blocked)**:

```json
{
  "status": "blocked",
  "address": "4oS78GPe66RqBduuAeiMFANf27FpmgXNwokZ3ocN4z1B",
  "reason": "Internal Security Alert: Address linked to Phishing Scam (Flagged manually)"
}
```

(The `reason` is the exact blocklist entry from the pre-seeded migration.)

#### Verify Cache Behavior

Call the same clean address twice:

1. First call: `"from_cache": false`
2. Second call (within 1 hour): `"from_cache": true`

#### Response Field Reference

| Field | Description |
|-------|-------------|
| `status` | `blocked` (in blocklist) or `analyzed` (checked external APIs) |
| `helius_assets_checked` | `true` if DAS check was performed, `false` if skipped (non-Helius RPC) |
| `has_sanctioned_assets` | Only meaningful when `helius_assets_checked: true` |
| `from_cache` | `true` if result came from database cache (< 1 hour old) |

---

## 4. Transaction Lifecycle and Database Operations

### State Machine Deep Dive

#### State Transitions by Component

| Transition | Triggered By | Code Location |
|------------|--------------|---------------|
| `pending` -> `pending_submission` | API Handler (after compliance approval) | `src/app/service.rs:submit_transfer()` |
| `pending_submission` -> `processing` | Worker (atomic claim) | `src/infra/database/postgres.rs:get_pending_blockchain_requests()` |
| `processing` -> `submitted` | Worker (successful RPC call) | `src/app/service.rs:process_single_submission()` |
| `processing` -> `pending_submission` | Worker (failed, retry eligible) | `src/app/service.rs:process_single_submission()` |
| `submitted` -> `confirmed` | Webhook Handler | `src/app/service.rs:process_helius_webhook()` |
| `submitted` -> `failed` | Webhook Handler (tx error) | `src/app/service.rs:process_helius_webhook()` |
| Any -> `failed` | Worker (max retries exceeded) | `src/app/service.rs:process_single_submission()` |

#### Compliance Status Transitions

| Transition | Triggered By |
|------------|--------------|
| `pending` -> `approved` | Range Protocol returns risk_score < threshold (default 6) and no risk text |
| `pending` -> `rejected` | Range Protocol returns risk_score >= threshold, or risk text match, or API error |

---

### Database Inspection Queries

#### Transaction Outbox Health Check

```sql
-- Count transactions by status
SELECT 
    blockchain_status,
    compliance_status,
    COUNT(*) as count
FROM transfer_requests
GROUP BY blockchain_status, compliance_status
ORDER BY count DESC;
```

#### Find Stuck Transactions

```sql
-- Transactions stuck in 'processing' for more than 5 minutes
SELECT id, from_address, blockchain_status, updated_at,
       NOW() - updated_at as stuck_duration
FROM transfer_requests
WHERE blockchain_status = 'processing'
  AND updated_at < NOW() - INTERVAL '5 minutes'
ORDER BY updated_at ASC;
```

#### Retry Statistics

```sql
-- Transactions with high retry counts
SELECT id, blockchain_retry_count, blockchain_last_error, 
       blockchain_next_retry_at, updated_at
FROM transfer_requests
WHERE blockchain_retry_count > 3
  AND blockchain_status != 'confirmed'
ORDER BY blockchain_retry_count DESC
LIMIT 20;
```

#### Submission Rate (Last Hour)

```sql
SELECT 
    date_trunc('minute', created_at) as minute,
    COUNT(*) as submissions
FROM transfer_requests
WHERE created_at > NOW() - INTERVAL '1 hour'
GROUP BY minute
ORDER BY minute DESC;
```

---

### Worker Claim Mechanism

The worker uses `FOR UPDATE SKIP LOCKED` to atomically claim tasks without race conditions when multiple replicas are running. Implementation: `src/infra/database/postgres.rs` → `get_pending_blockchain_requests`. The actual query uses bound parameters `$1` (now) and `$2` (batch size); conceptually:

```sql
UPDATE transfer_requests
SET blockchain_status = 'processing',
    updated_at = NOW()
WHERE id IN (
    SELECT id FROM transfer_requests
    WHERE blockchain_status = 'pending_submission'
      AND compliance_status = 'approved'
      AND (blockchain_next_retry_at IS NULL OR blockchain_next_retry_at <= NOW())
      AND blockchain_retry_count < 10
    ORDER BY blockchain_next_retry_at ASC NULLS FIRST, created_at ASC
    LIMIT 10
    FOR UPDATE SKIP LOCKED
)
RETURNING *;
```

The `LIMIT` is the worker `batch_size` (default 10, from `WorkerConfig` in `src/app/worker.rs`). This ensures:
1. Only eligible transactions are selected
2. Locked rows are skipped (preventing duplicate processing)
3. Status is atomically updated before returning

---

## 5. Comprehensive Troubleshooting

| Symptom | Root Cause | Solution |
|---------|------------|----------|
| `401 Unauthorized` on webhook | `HELIUS_WEBHOOK_SECRET` mismatch | The backend compares the raw `Authorization` header value to `HELIUS_WEBHOOK_SECRET`. Ensure Helius sends exactly that string (no `Bearer ` prefix) and the env var matches. |
| `Authorization error: Signature verification failed` | Message format mismatch | Compare signing message bytes between WASM and backend; check for encoding differences |
| `module-not-found` WASM error | Next.js not finding .wasm file | Rebuild WASM, delete `.next` directory, configure `next.config.js` for WASM |
| `429 Too Many Requests` from RPC | Rate limit exceeded | Implement request throttling; upgrade to paid tier; use QuickNode/Helius |
| `pool timed out while waiting for an open connection` | PostgreSQL connection exhaustion | Increase `max_connections` in `PostgresConfig`; add Railway PostgreSQL replicas |
| Transactions stuck in `processing` | Worker crashed mid-processing | Manually reset: `UPDATE transfer_requests SET blockchain_status = 'pending_submission' WHERE blockchain_status = 'processing' AND updated_at < NOW() - INTERVAL '10 minutes'` |
| Webhook received but not processed | Signature not in database | Verify relayer wallet is in Helius webhook's Account Addresses |
| `Blockhash not found` | Transaction expired before confirmation | Increase retry speed; use `skip_preflight: true`; network may be congested |
| Compliance always `rejected` | Range API error defaulting to rejection | Check Range API key validity; check network connectivity to `api.range.org` |
| Background worker not processing | Worker disabled or crashed | Verify `ENABLE_BACKGROUND_WORKER=true`; check logs for panic/error |

---

## 6. Security and Performance Tuning

### Rotating the ISSUER_PRIVATE_KEY

> **CAUTION**: Key rotation requires careful coordination to avoid transaction failures.

1. **Generate a new keypair**:
   ```bash
   solana-keygen new --outfile new-relayer-keypair.json
   ```

2. **Fund the new wallet**:
   ```bash
   solana transfer NEW_PUBKEY 1 --from OLD_KEYPAIR
   ```

3. **Update Helius webhook** to include the new public key in Account Addresses.

4. **Export the new key as Base58**: The relayer expects `ISSUER_PRIVATE_KEY` as a Base58-encoded private key. The keypair JSON is a 64-byte array; use a secure method to convert to Base58 (e.g. a small script with `bs58` or your key-management tool). **Never commit raw keypair files.**

5. **Deploy with new key**: Set `ISSUER_PRIVATE_KEY` to the Base58 value in your environment (Railway, etc.).

6. **Verify operation** before removing the old key from the webhook.

7. **Remove old key** from Helius webhook Account Addresses.

---

### Adjusting Background Worker Parameters

**Environment Variables**:

| Variable | Default | Description |
|----------|---------|-------------|
| `ENABLE_BACKGROUND_WORKER` | `true` | Enable/disable the worker |
| `ENABLE_PRIVACY_CHECKS` | `true` | Enable anonymity set checks for confidential transfers |

**Code Configuration** (in `src/app/worker.rs`):

```rust
impl Default for WorkerConfig {
    fn default() -> Self {
        Self {
            poll_interval: Duration::from_secs(10),  // How often to check for pending txs
            batch_size: 10,                          // Max txs to process per cycle (worker claim LIMIT)
            enabled: true,
            enable_privacy_checks: true,
        }
    }
}
```

`poll_interval` and `batch_size` are **not** read from environment variables; they are set in code. To tune in production, change defaults in `WorkerConfig` and redeploy, or add env parsing (e.g. `WORKER_POLL_INTERVAL_SECS`, `WORKER_BATCH_SIZE`).

---

### Rate Limiting Configuration

**Environment Variables**:

| Variable | Default | Description |
|----------|---------|-------------|
| `ENABLE_RATE_LIMITING` | `false` | Enable Governor middleware |
| `RATE_LIMIT_RPS` | `10` | Requests per second (general endpoints) |
| `RATE_LIMIT_BURST` | `20` | Burst allowance before throttling |

**Health endpoints** have separate limits (100 RPS) to support Kubernetes probes without affecting application traffic.

---

### PostgreSQL Connection Pool Tuning

Pool configuration is **code-only** (no environment variables). Modify `PostgresConfig` in `src/infra/database/postgres.rs`:

```rust
impl Default for PostgresConfig {
    fn default() -> Self {
        Self {
            max_connections: 10,              // Increase for high throughput
            min_connections: 2,               // Keep warm connections
            acquire_timeout: Duration::from_secs(3),  // Fail fast on pool exhaustion
            idle_timeout: Duration::from_secs(600),   // 10 min idle before close
            max_lifetime: Duration::from_secs(1800),  // 30 min max connection age
        }
    }
}
```

The pool is created in `PostgresClient::new()` via `PgPoolOptions`. To tune via env (e.g. `PG_MAX_CONNECTIONS`), add parsing and pass a custom `PostgresConfig`.

**Railway PostgreSQL Recommendations**:
- **Starter**: max_connections = 5-10
- **Pro**: max_connections = 20-50
- **Scale**: max_connections = 100+

---

## Appendix: Quick Reference

### Environment Variables Summary

| Variable | Required | Default | Description |
|----------|----------|---------|-------------|
| `DATABASE_URL` | Yes | - | PostgreSQL connection string |
| `SOLANA_RPC_URL` | No | `https://api.devnet.solana.com` | Solana RPC endpoint (Helius/QuickNode recommended) |
| `ISSUER_PRIVATE_KEY` | Yes | - | Base58 relayer wallet key |
| `RANGE_API_KEY` | No | - | Range Protocol key (mock if absent) |
| `RANGE_API_URL` | No | `https://api.range.org/v1` | Override Range API base URL |
| `RANGE_RISK_THRESHOLD` | No | `6` | Risk threshold (1-10); ≥ threshold = reject |
| `HELIUS_WEBHOOK_SECRET` | No | - | Exact `Authorization` header value for Helius webhooks |
| `ENABLE_RATE_LIMITING` | No | `false` | Governor middleware toggle |
| `RATE_LIMIT_RPS` | No | `10` | Requests per second (general endpoints) |
| `RATE_LIMIT_BURST` | No | `20` | Burst size before throttling |
| `ENABLE_BACKGROUND_WORKER` | No | `true` | Worker process toggle |
| `ENABLE_PRIVACY_CHECKS` | No | `true` | QuickNode Privacy Health Check for confidential transfers |
| `HOST` | No | `0.0.0.0` | Bind address |
| `PORT` | No | `3000` | Bind port |
| `CORS_ALLOWED_ORIGINS` | No | (see `.env.example`) | Comma-separated CORS origins |

### Key File Locations

| Purpose | Path |
|---------|------|
| Application config (env parsing) | `src/main.rs` (`Config` struct) |
| API Handlers | `src/api/handlers.rs` |
| Business Logic | `src/app/service.rs` |
| Risk Check Service | `src/app/risk_service.rs` |
| Background Worker | `src/app/worker.rs` |
| Database Queries | `src/infra/database/postgres.rs` |
| Blockchain Client | `src/infra/blockchain/solana.rs` |
| Compliance Provider | `src/infra/compliance/range.rs` |
| Provider Strategies | `src/infra/blockchain/strategies.rs` |
| Signing message format | `src/domain/types.rs` (`SubmitTransferRequest::create_signing_message`) |
| Migrations | `migrations/*.sql` |

# Security

This document details the security hardening features implemented in the Solana Compliance Relayer, including MEV protection, double-spend prevention, replay attack protection, and the internal blocklist manager.

---

## Table of Contents

- [Jito Bundle Integration (MEV Protection)](#jito-bundle-integration-mev-protection)
- [Double-Spend Protection](#double-spend-protection)
- [Replay Attack Protection](#replay-attack-protection)
- [Internal Blocklist Manager](#internal-blocklist-manager)
- [Enterprise Security Summary](#enterprise-security-summary)

---

## Jito Bundle Integration (MEV Protection)

When using QuickNode with Jito bundles enabled, transactions are submitted privately to Jito block builders, **bypassing the public mempool**. This provides protection against:

- **Frontrunning:** Attackers cannot see your transaction before it's included
- **Sandwich Attacks:** No opportunity to place transactions around yours
- **MEV Extraction:** Your transaction value stays with you

### How It Works

```
┌─────────────────────────────────────────────────────────────────────────┐
│                    Standard Submission (Public Mempool)                 │
│  Transaction → Public Mempool → Visible to MEV Bots → Block Inclusion   │
│                           VULNERABLE TO ATTACKS                         │
└─────────────────────────────────────────────────────────────────────────┘

┌─────────────────────────────────────────────────────────────────────────┐
│                    Jito Bundle Submission (Private)                     │
│  Transaction + Tip → Jito Block Builder → Direct Block Inclusion        │
│                           MEV PROTECTED                                 │
└─────────────────────────────────────────────────────────────────────────┘
```

### Automatic Tip Injection

When Jito is enabled, the relayer **automatically appends a tip instruction** to each transaction before signing:

1. A **random** Jito tip account is selected from the 8 official Jito tip accounts (via `random_jito_tip_account()`) to reduce contention
2. A SOL transfer instruction is added as the **last instruction** in the transaction (in `RpcBlockchainClient::create_jito_tip_instruction`)
3. The tip amount is configurable via `JITO_TIP_LAMPORTS` (default: 1,000 lamports = 0.000001 SOL; recommended for production: 10,000–50,000)

### Fail-Safe Design

The Jito integration implements a strict **no-leak guarantee**:

- If Jito submission fails, the transaction is **NOT** sent to the public mempool
- The `QuickNodePrivateSubmissionStrategy` returns an error on Jito failure and does **not** fall back to `sendTransaction`
- This prevents accidental MEV exposure on Jito failures
- Failed submissions return an error for upstream retry logic

### Configuration

```env
# Enable Jito bundle submission (requires QuickNode with "Lil' JIT" add-on)
USE_JITO_BUNDLES=true

# Tip amount in lamports (default: 1,000; recommended: 10,000–50,000)
JITO_TIP_LAMPORTS=10000

# Optional: Specify region for lower latency (ny, amsterdam, frankfurt, tokyo)
JITO_REGION=ny
```

### Requirements

1. **QuickNode RPC endpoint** with the ["Lil' JIT - JITO Bundles and transactions"](https://marketplace.quicknode.com/add-on/lil-jit-jito-bundles-and-transactions) add-on enabled
2. `USE_JITO_BUNDLES=true` in environment
3. Sufficient SOL balance for transaction fees + tip

---

## Double-Spend Protection

When a Jito bundle submission returns an ambiguous state (`JitoStateUnknown`), the relayer implements **status-aware retry logic** to prevent double-spend scenarios.

### The Problem

```
┌─────────────────────────────────────────────────────────────────────────┐
│                    Without Double-Spend Protection                      │
│                                                                         │
│  Submit TX → Jito Timeout → Retry with NEW blockhash → DOUBLE SPEND!    │
│             (original may have actually landed)                         │
└─────────────────────────────────────────────────────────────────────────┘
```

### The Solution

```
┌─────────────────────────────────────────────────────────────────────────┐
│                    With Double-Spend Protection                         │
│                                                                         │
│  Submit TX → Jito Timeout → Query getSignatureStatuses(original_sig)    │
│                                       │                                 │
│               ┌───────────────────────┼───────────────────────┐         │
│               ▼                       ▼                       ▼         │
│           Confirmed?              Not Found?                Failed?     │
│           Mark SUCCESS         Check Blockhash              Safe to     │
│           (no retry!)             Expired?                  Retry       │
│                                     │                                   │
│                           ┌─────────┴─────────┐                         │
│                           ▼                   ▼                         │
│                      Still Valid?         Expired?                      │
│                      Wait longer      Safe to retry                     │
│                      (backoff)       (new blockhash)                    │
└─────────────────────────────────────────────────────────────────────────┘
```

### How It Works

1. **Original Signature Tracking:** When a transaction is first submitted (including to Jito), the transaction signature is extracted from the serialized tx before submission; on success or ambiguous error, the signature and blockhash are stored in the database (`original_tx_signature`, `blockhash_used`).

2. **Error Classification:** Errors are classified into types (`LastErrorType`):
   - `JitoStateUnknown` — Ambiguous state, MUST check status before retry
   - `JitoBundleFailed` — Definite failure, safe to retry
   - `TransactionFailed` — On-chain failure, safe to retry
   - `NetworkError` — Connection/issues, safe to retry
   - `ValidationError` — Request validation failure

3. **Status Verification:** Before retrying after a `JitoStateUnknown` error (in `process_single_submission` and manual retry handler):
   - Query the blockchain client for the original transaction signature (RPC: `getSignatureStatuses`)
   - If **Confirmed/Finalized:** Mark as success, no retry needed
   - If **Failed:** Safe to retry with new blockhash
   - If **Not Found:** Check if blockhash has expired
     - Blockhash still valid: Wait longer with exponential backoff (do not submit new tx)
     - Blockhash expired: Safe to retry with new blockhash
   - If **RPC/network error** when checking status: Do not submit a new transaction; reschedule for a later status check only

### Database Tracking

The relayer stores three additional fields for each transfer request (migration `20260129000000_add_jito_retry_tracking.sql`):

| Field | Purpose |
|-------|---------|
| `original_tx_signature` | First signature (from initial submission), used for status verification before retry |
| `last_error_type` | Classification of last error for smart retry logic |
| `blockhash_used` | Blockhash from last attempt, for expiry checking |

---

## Replay Attack Protection

The relayer enforces request uniqueness through a dual mechanism: **nonces** and **idempotency keys**.

### Nonce-Based Protection

Every `POST /transfer-requests` request must include a **nonce** in the body, and the **signature must be computed over a message that includes the nonce**.

| Requirement | Details |
|-------------|---------|
| **Format** | 32–64 characters, alphanumeric with optional hyphens (UUID format) |
| **Recommended** | UUID v4 or UUID v7 (time-ordered) |
| **Enforcement** | Server rejects duplicate nonces (idempotent return for same nonce per from_address) |
| **Validation** | Nonce is required; empty or invalid length/characters are rejected |

### Signing Message Format

The signed message **must** include the nonce. The server builds the message as `{from}:{to}:{amount|confidential}:{mint|SOL}:{nonce}` and verifies the signature against it (`create_signing_message` / `verify_signature` in `SubmitTransferRequest`).

| Version | Format |
|---------|--------|
| **Current (required)** | `{from}:{to}:{amount}:{mint}:{nonce}` (or `confidential` for amount in confidential transfers) |

Example:

```
7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU:RecipientPubkey...:1000000000:SOL:019470a4-7e7c-7d3e-8f1a-2b3c4d5e6f7a
```

### API Idempotency

The `Idempotency-Key` header enables safe retries:

- If a request with the same nonce already exists for the same `from_address`, the server returns the existing transfer (200 OK)
- When present, `Idempotency-Key` must equal the body `nonce`
- Duplicate requests return the original response without creating a new transfer

---

## Internal Blocklist Manager

The relayer includes a high-performance internal blocklist that acts as a "hot cache" for screening malicious addresses **before** querying external compliance providers like Range Protocol.

### Architecture

```
┌─────────────────────────────────────────────────────────────────────┐
│                     Address Screening Pipeline                      │
├─────────────────────────────────────────────────────────────────────┤
│                                                                     │
│  ┌──────────────────┐    ┌──────────────────┐     ┌───────────────┐ │
│  │  Internal        │    │  Range Protocol  │     │  Blockchain   │ │
│  │  Blocklist       │───▶│  Risk API        │───▶│  Submission   │ │
│  │  (DashMap O(1))  │    │  (Network call)  │     │               │ │
│  └──────────────────┘    └──────────────────┘     └───────────────┘ │
│         │                         │                                 │
│         ▼                         ▼                                 │
│    Instant reject           Risk score ≥ threshold                  │
│    (no API call)            (default: 6, scale 1–10) = Rejected     │
│                                    │                                │
│                                    ▼                                │
│                            Auto-add to blocklist                    │
│                            (future requests rejected instantly)    │
└─────────────────────────────────────────────────────────────────────┘
```

### Features

| Feature | Description |
|---------|-------------|
| **Thread-Safe** | Uses `dashmap::DashMap` for lock-free concurrent access |
| **Persistent** | All changes are persisted to PostgreSQL and survive restarts |
| **O(1) Lookups** | In-memory cache provides instant address checks |
| **Admin API** | Real-time management via HTTP endpoints |
| **Dual Check** | Both sender and recipient addresses are screened |

### Admin API Usage

```bash
# Add an address to the blocklist (JSON body)
curl -X POST http://localhost:3000/admin/blocklist \
  -H "Content-Type: application/json" \
  -d '{
    "address": "SuspiciousWallet123...",
    "reason": "Suspected phishing activity"
  }'

# List all blocklisted addresses
curl http://localhost:3000/admin/blocklist

# Remove an address from the blocklist
curl -X DELETE http://localhost:3000/admin/blocklist/SuspiciousWallet123...
```

### Pre-Seeded Blocklist

The system initializes with a seeded blocklist entry (inserted by migration `20260121000000_create_blocklist_table.sql`):

| Address | Reason |
|---------|--------|
| `4oS78GPe66RqBduuAeiMFANf27FpmgXNwokZ3ocN4z1B` | Internal Security Alert: Address linked to Phishing Scam (Flagged manually) |

### Auto-Block on High Risk

When Range Protocol returns a high-risk score (≥ configured threshold, default 6), the address is **automatically added** to the internal blocklist (in `AppService::create_transfer_request` and in `RiskService::check_address`). Future requests involving that address are rejected instantly without another Range API call.

---

## Enterprise Security Summary

| Feature | Description |
|---------|-------------|
| **MEV-Protected Transactions ("Ghost Mode")** | Transactions are submitted privately via Jito Bundles (QuickNode), preventing front-running and sandwich attacks. No fallback to public mempool on Jito failure. |
| **Double-Spend Protection** | Retry logic queries on-chain status (`getSignatureStatuses`) for the original signature before re-broadcasting after `JitoStateUnknown`. Reschedules instead of retrying when status cannot be verified. |
| **Smart Rent Recovery** | For confidential (ZK) transfers, the relayer closes ephemeral ZK-proof context accounts after the transfer, recovering rent-exempt lamports to the relayer. |
| **Dual-Confirmation System** | Real-time transaction status updates via QuickNode Streams (Webhooks) and Helius Enhanced Webhooks; stale-transaction crank polls `getSignatureStatuses` when webhooks are missed. |
| **Replay Attack Protection** | Cryptographic nonces in signed messages (`{from}:{to}:{amount}:{mint}:{nonce}`) prevent request replay; idempotency keys enable safe retries. |
| **Multi-Layer Compliance** | Internal blocklist (DashMap + PostgreSQL) + Range Protocol + Helius DAS; high-risk addresses from Range are auto-added to the blocklist. |

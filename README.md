<div align="center">

# Solana Compliance Relayer

### Bridging the gap between on-chain privacy, regulatory compliance, and high-throughput execution.

[![Rust](https://img.shields.io/badge/Rust-000000?style=for-the-badge&logo=rust&logoColor=white)](https://www.rust-lang.org/)
[![PostgreSQL](https://img.shields.io/badge/PostgreSQL-4169E1?style=for-the-badge&logo=postgresql&logoColor=white)](https://www.postgresql.org/)
[![Solana](https://img.shields.io/badge/Solana-9945FF?style=for-the-badge&logo=solana&logoColor=white)](https://solana.com/)
[![Jito MEV Protection](https://img.shields.io/badge/Jito-MEV%20Protected-10B981?style=for-the-badge&logo=shield&logoColor=white)](https://www.jito.wtf/)
[![Helius](https://img.shields.io/badge/Helius-FF5733?style=for-the-badge&logo=data:image/svg+xml;base64,PHN2ZyB4bWxucz0iaHR0cDovL3d3dy53My5vcmcvMjAwMC9zdmciIHZpZXdCb3g9IjAgMCAyNCAyNCI+PC9zdmc+&logoColor=white)](https://helius.dev/)
[![QuickNode](https://img.shields.io/badge/QuickNode-195AD2?style=for-the-badge&logo=quicknode&logoColor=white)](https://www.quicknode.com/)
[![Range Protocol](https://img.shields.io/badge/Range%20Protocol-6D28D9?style=for-the-badge&logo=shield&logoColor=white)](https://www.rangeprotocol.com/)
[![License: MIT](https://img.shields.io/badge/License-MIT-22C55E?style=for-the-badge)](LICENSE)
[![Author](https://img.shields.io/badge/Author-Berektassuly.com-F97316?style=for-the-badge)](https://berektassuly.com)
</div>

---

## Table of Contents

- [Why This Exists](#why-this-exists)
- [How It Works](#how-it-works)
- [Architecture](#architecture)
- [Key Features](#key-features)
- [Technical Stack](#technical-stack)
- [RPC Provider Strategy](#rpc-provider-strategy)
- [Jito Bundle Integration (MEV Protection)](#jito-bundle-integration-mev-protection)
- [Double-Spend Protection](#double-spend-protection)
- [Internal Blocklist Manager](#internal-blocklist-manager)
- [Transaction Lifecycle](#transaction-lifecycle)
- [Getting Started](#getting-started)
- [Environment Configuration](#environment-configuration)
- [API Reference](#api-reference)
- [Client Integration / SDK](#client-integration--sdk)
- [CLI Tools](#cli-tools)
- [Testing](#testing)
- [Deployment](#deployment)
- [Roadmap](#roadmap)
- [Contact](#contact)
- [Guide](docs/GUIDE.md)
- [Contributing](CONTRIBUTING.md)
- [License](LICENSE)

---

## Why This Exists

Privacy-preserving protocols on Solana face a fundamental paradox: **users demand confidentiality**, but **institutions require auditability**. The Solana Compliance Relayer resolves this tension through a **Defense-in-Depth** architecture that:

| Challenge | Solution |
|-----------|----------|
| Blinded signing risk | Client-side WASM signing ensures wallets never expose private keys to the server |
| Regulatory compliance | Real-time AML/Sanctions screening via Range Protocol before chain submission |
| Transaction guarantees | Transactional Outbox pattern with PostgreSQL ensures no approved tx is ever lost |
| Finalization visibility | Helius Enhanced Webhooks notify the system when transactions are confirmed |

> **Core Guarantee:** Rejected transactions are persisted for audit but **never** submitted to the blockchain.

---

## How It Works

The system implements a **three-stage pipeline**:

1. **Client-Side Cryptography (WASM)**: The browser compiles Rust-based Ed25519 signing logic to WebAssembly. Users sign transaction intent locally, eliminating private key exposure.

2. **Compliance Gate (Range Protocol)**: Before any blockchain call, wallet addresses are screened against sanctions lists, PEP databases, and on-chain risk signals.

3. **Execution & Finalization (Helius / QuickNode)**: Approved transactions are submitted via Helius or QuickNode RPC with priority fee optimization. Helius Enhanced Webhooks (or provider-specific callbacks) provide real-time confirmation.

---

## Architecture

This project implements **Hexagonal Architecture** (Ports and Adapters), ensuring clean separation between business logic and infrastructure concerns.

```
┌─────────────────────────────────────────────────────────────────────────┐
│                           FRONTEND (Next.js)                            │
│  ┌───────────────────┐    ┌──────────────────┐    ┌─────────────────┐   │
│  │   Terminal Panel  │    │  WASM Signer     │    │  Monitor Panel  │   │
│  │   (Transfer UI)   │──▶│  (Ed25519-dalek) │    │  (5s Polling)   │   │
│  └───────────────────┘    └────────┬─────────┘    └─────────────────┘   │
└────────────────────────────────────┼────────────────────────────────────┘
                                     │ Signed Request
                                     ▼
┌─────────────────────────────────────────────────────────────────────────┐
│                           BACKEND (Axum + Rust)                         │
│  ┌─────────────────────────────────────────────────────────────────┐    │
│  │                        API Layer                                │    │
│  │  POST /transfer-requests  │  GET /transfer-requests/{id}        │    │
│  │  POST /webhooks/helius    │  GET /health, /health/live, /ready  │    │
│  │  POST /risk-check         │  /admin/blocklist (CRUD)            │    │
│  └─────────────────────────────┬───────────────────────────────────┘    │
│                                 │                                       │
│  ┌──────────────────────────────▼──────────────────────────────────┐    │
│  │                      Application Layer                          │    │
│  │  ┌─────────────┐    ┌───────────────────┐   ┌──────────────────┐│    │
│  │  │ AppService  │──▶│ ComplianceProvider│──▶│ DatabaseClient   ││    │
│  │  └─────────────┘    │ (Range Protocol)  │   │ (PostgreSQL)     ││    │
│  │                     └───────────────────┘   └──────────────────┘│    │
│  └─────────────────────────────────────────────────────────────────┘    │
│                                 │                                       │
│  ┌──────────────────────────────▼──────────────────────────────────┐    │
│  │                    Infrastructure Layer                         │    │
│  │  ┌──────────────────┐   ┌───────────────────┐                   │    │
│  │  │ Background Worker│──▶│ BlockchainClient  │──▶ Helius RPC    │    │
│  │  │ (10s poll cycle) │   │ (Strategy Pattern)│                   │    │
│  │  └──────────────────┘   └───────────────────┘                   │    │
│  └─────────────────────────────────────────────────────────────────┘    │
└─────────────────────────────────────────────────────────────────────────┘
```

### Directory Structure

```
src/
├── domain/          # Core business types and trait definitions (Ports)
│   ├── types.rs     # TransferRequest, ComplianceStatus, BlockchainStatus
│   ├── traits.rs    # DatabaseClient, BlockchainClient, ComplianceProvider
│   └── error.rs     # Unified error types
├── app/             # Application layer (Use Cases)
│   ├── service.rs   # Business logic orchestration
│   ├── state.rs     # AppState (service, blocklist, risk_service, etc.)
│   ├── risk_service.rs  # Pre-flight risk check (blocklist + Range + DAS)
│   └── worker.rs    # Background retry worker with exponential backoff
├── api/             # HTTP interface (Primary Adapter)
│   ├── handlers.rs  # Axum route handlers with OpenAPI docs
│   ├── admin.rs     # Admin API for blocklist management
│   └── router.rs    # Rate limiting, CORS, middleware
└── infra/           # External integrations (Secondary Adapters)
    ├── database/    # PostgreSQL via SQLx (compile-time checked)
    ├── blockchain/  # Solana via Helius/QuickNode/Standard RPC (strategies)
    ├── blocklist/   # Internal blocklist with DashMap + PostgreSQL
    ├── compliance/  # Range Protocol integration
    └── privacy/     # QuickNode Privacy Health Check (confidential transfers)
```

---

## Data Flow Sequence

```mermaid
sequenceDiagram
    participant User as User Browser
    participant WASM as WASM Signer
    participant API as Axum API
    participant Range as Range Protocol
    participant DB as PostgreSQL
    participant Worker as Background Worker
    participant Helius as Helius RPC
    participant Webhook as Helius Webhook

    User->>WASM: Initiate Transfer
    WASM->>WASM: Ed25519 Sign (Client-Side)
    WASM->>API: POST /transfer-requests (Signed Payload + nonce)
    
    API->>API: Verify Ed25519 Signature (message includes nonce)
    API->>API: Check Idempotency (existing nonce → return existing)
    API->>API: Check Internal Blocklist (DashMap)
    
    alt Address in Internal Blocklist
        API->>DB: Persist (status: rejected, error: "Blocklist: reason")
        API-->>User: 200 OK {blockchain_status: "failed"}
    else Address Not Blocked
        API->>Range: check_compliance(address)
        alt Address High Risk (score >= threshold)
            Range-->>API: Rejected (CRITICAL/HIGH risk)
            API->>DB: Persist (compliance_status: rejected)
            API->>API: Auto-add to Internal Blocklist
            API-->>User: 200 OK {blockchain_status: "failed"}
        else Address Clean
            Range-->>API: Approved (riskScore < threshold)
            API->>DB: Persist (compliance_status: approved, blockchain_status: pending_submission)
            API-->>User: 200 OK {status: "pending_submission"}
        end
    end

    loop Every 10 seconds
        Worker->>DB: Poll (blockchain_status = pending_submission AND compliance_status = approved)
        Worker->>DB: Update (blockchain_status: processing)
        Worker->>Helius: sendTransaction()
        
        alt Submission Success
            Helius-->>Worker: Transaction Signature
            Worker->>DB: Update (blockchain_status: submitted, signature: <sig>)
        else Submission Failure
            Worker->>DB: Increment retry_count, calculate exponential backoff
            Note over Worker,DB: Status remains pending_submission until max retries (10)
        end
    end

    Helius->>Helius: Transaction Finalized on Solana
    Helius->>Webhook: POST /webhooks/helius (Enhanced Transaction)
    Webhook->>API: Receive Confirmation
    API->>DB: Update (blockchain_status: confirmed)
    
    Note over User,DB: Frontend polls GET /transfer-requests every 5s to reflect final status
```

---

## Key Features

| Feature | Description |
|-----------|-------------|
| **Client-Side WASM Signing** | Ed25519 via `ed25519-dalek` compiled to WebAssembly—private keys never leave the browser |
| **Replay Attack Protection** | Cryptographic enforcement of request uniqueness via **nonces**. Every request must include a unique nonce in the body and in the signed message; the server rejects duplicates and replays. |
| **API Idempotency** | Safe retries using **Idempotency-Key** headers to prevent duplicate processing. Resend the same request with the same key to receive the original response without creating a second transfer. |
| **MEV Protection (Jito Bundles)** | Private transaction submission via Jito block builders, bypassing the public mempool |
| **Real-Time Transaction Monitoring** | Frontend polls API every 5 seconds with TanStack Query |
| **Pre-Flight Risk Check** | `POST /risk-check` aggregates blocklist, Range Protocol, and Helius DAS data with 1-hour caching |
| **Internal Blocklist Manager** | Thread-safe DashMap cache with PostgreSQL persistence for fast local address screening |
| **Automated AML/Compliance Screening** | Range Protocol Risk API with configurable threshold (default: 6 = High Risk) |
| **Public & Confidential Transfers** | Supports standard SOL/SPL and Token-2022 ZK confidential transfers |
| **Resilient Background Worker** | Exponential backoff retries (up to 10 attempts, max 5-minute delay) |
| **Helius Webhook Integration** | Real-time finalization callbacks move transactions from `submitted` -> `confirmed` |
| **Provider Strategy Pattern** | Auto-detects Helius/QuickNode for premium features (priority fees, DAS, Jito) |
| **Rate Limiting** | Governor-based middleware with configurable RPS and burst limits |
| **OpenAPI Documentation** | Auto-generated Swagger UI at `/swagger-ui` |

### Enterprise-Grade Security Features

| Feature | Description |
|-----------|-------------|
| **MEV-Protected Transactions ("Ghost Mode")** | Transactions are submitted privately via Jito Bundles, preventing front-running and sandwich attacks. Your transaction value stays with you. |
| **Double-Spend Protection** | Advanced retry mechanism that queries on-chain status (`getSignatureStatuses`) before re-broadcasting after ambiguous failures. Prevents losses during network timeouts or Jito `StateUnknown` responses. |
| **Smart Rent Recovery** | Automatically closes ephemeral ZK-proof context accounts after confidential transfers, recovering ~0.002-0.01 SOL per transaction that would otherwise be permanently locked. |
| **Dual-Confirmation System** | Real-time transaction status updates via QuickNode Streams (Webhooks) and Helius Enhanced Webhooks for instant finalization visibility. |

---

## Technical Stack

### Backend

| Component | Technology |
|-----------|------------|
| Language | Rust 1.85+ (2024 edition) |
| Web Framework | Axum 0.8 |
| Database | PostgreSQL 16+ (SQLx 0.8, compile-time verification) |
| Async Runtime | Tokio 1.48 |
| HTTP Client | Reqwest 0.12 |
| Rate Limiting | Governor 0.10 |
| API Docs | utoipa 5, utoipa-swagger-ui 9 (OpenAPI 3.0) |
| Middleware | Tower 0.5, Tower-HTTP 0.6 (tracing, timeout, CORS) |

### Frontend

The frontend lives in a [separate repository](https://github.com/Berektassuly/solana-compliance-relayer-frontend).

| Component | Technology |
|-----------|------------|
| Framework | Next.js 14 (App Router) |
| Styling | Tailwind CSS |
| State Management | TanStack Query (React Query) |
| Build | Turbopack |

### Cryptography

| Component | Technology |
|-----------|------------|
| Signing | ed25519-dalek 2.1 (WASM-compiled) |
| ZK Proofs | solana-zk-sdk 4.0, spl-token-confidential-transfer-proof-generation 0.5 |
| Encryption | ElGamal, AES (solana-zk-sdk) |

### Infrastructure

| Component | Technology |
|-----------|------------|
| RPC Provider | Helius / QuickNode (auto-detected via URL) |
| Compliance | Range Protocol Risk API |
| Internal Blocklist | DashMap 6 (O(1) cache) + PostgreSQL |
| Deployment (Backend) | Railway / Docker |
| Deployment (Frontend) | Vercel |
| Database Hosting | Railway PostgreSQL |

---

## RPC Provider Strategy

The relayer implements a **Provider Strategy Pattern** that auto-detects the RPC endpoint and activates premium features accordingly:

| Provider | Detection | Features |
|----------|-----------|----------|
| **Helius** | URL contains `helius-rpc.com` | Priority fee estimation via `getPriorityFeeEstimate`, DAS compliance checks, Enhanced Webhooks |
| **QuickNode** | URL contains `quiknode.pro` or `quicknode.com` | Priority fee estimation via `qn_estimatePriorityFees`, Privacy Health Check service, **Jito Bundle Submission (MEV Protection)** |
| **Standard** | Any other RPC | Static fallback fee strategy (5000 micro-lamports) |

### QuickNode-Specific Features

- **Priority Fee Estimation**: Uses the `qn_estimatePriorityFees` RPC method to fetch real-time fee recommendations
- **Privacy Health Check Service**: Monitors token activity to recommend optimal submission timing for confidential transfers
- **Jito Bundle Submission (Ghost Mode)**: MEV-protected private transaction submission via Jito block builders

### Configuration Examples

```env
# Helius (recommended for webhooks)
SOLANA_RPC_URL=https://mainnet.helius-rpc.com/?api-key=YOUR_HELIUS_KEY

# QuickNode (recommended for MEV protection + privacy features)
SOLANA_RPC_URL=https://your-endpoint.solana-mainnet.quiknode.pro/YOUR_API_KEY

# Standard RPC (development only)
SOLANA_RPC_URL=https://api.devnet.solana.com
```

---

## Jito Bundle Integration (MEV Protection)

When using QuickNode with Jito bundles enabled, transactions are submitted privately to Jito block builders, **bypassing the public mempool**. This provides protection against:

- **Frontrunning**: Attackers cannot see your transaction before it's included
- **Sandwich Attacks**: No opportunity to place transactions around yours
- **MEV Extraction**: Your transaction value stays with you

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

1. A random Jito tip account is selected (from 8 official accounts) to reduce contention
2. A SOL transfer instruction is added as the **last instruction** in the transaction
3. The tip amount is configurable via `JITO_TIP_LAMPORTS` (default: 10,000 = 0.00001 SOL)

### Fail-Safe Design

The Jito integration implements a strict **no-leak guarantee**:

- If Jito submission fails, the transaction is **NOT** sent to the public mempool
- This prevents accidental MEV exposure on Jito failures
- Failed submissions return an error for upstream retry logic

### Configuration

```env
# Enable Jito bundle submission (requires QuickNode with "Lil' JIT" add-on)
USE_JITO_BUNDLES=true

# Tip amount in lamports (minimum: 1,000, recommended: 10,000-50,000)
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

1. **Original Signature Tracking**: When a transaction is first submitted, the signature and blockhash are stored in the database.

2. **Error Classification**: Errors are classified into types:
   - `JitoStateUnknown` - Ambiguous state, MUST check status before retry
   - `JitoBundleFailed` - Definite failure, safe to retry
   - `TransactionFailed` - On-chain failure, safe to retry
   - `NetworkError` - Connection issues, safe to retry

3. **Status Verification**: Before retrying a `JitoStateUnknown` error:
   - Query `getSignatureStatuses` for the original transaction
   - If **Confirmed/Finalized**: Mark as success, no retry needed
   - If **Failed**: Safe to retry with new blockhash
   - If **Not Found**: Check if blockhash has expired (~150 slots)
     - Blockhash valid: Wait longer with exponential backoff
     - Blockhash expired: Safe to retry with new blockhash

### Database Tracking

The relayer stores three additional fields for each transaction:

| Field | Purpose |
|-------|---------|
| `original_tx_signature` | First signature used, for status verification |
| `last_error_type` | Classification of last error for smart retry logic |
| `blockhash_used` | Blockhash from last attempt, for expiry checking |

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
│                                                                     │
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
# Add an address to the blocklist
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

The system initializes with a seeded blocklist entry for demonstration:

| Address | Reason |
|---------|--------|
| `4oS78GPe66RqBduuAeiMFANf27FpmgXNwokZ3ocN4z1B` | Internal Security Alert: Address linked to Phishing Scam (Flagged manually) |

---

## Transaction Lifecycle

Transactions progress through the following states:

```
┌─────────┐    ┌───────────────────┐    ┌────────────┐    ┌───────────┐    ┌───────────┐
│ Pending │──▶│ PendingSubmission │───▶│ Processing │──▶│ Submitted │───▶│ Confirmed │
└─────────┘    └───────────────────┘    └────────────┘    └───────────┘    └───────────┘
                        │                      │                                  │
                        │                      │                                  │
                        ▼                      ▼                                  │
                   ┌──────────┐           ┌─────────┐                             │
                   │  Failed  │◀─────────│  Retry  │◀────────────────────────────┘
                   │(10 tries)│           │(backoff)│    (if webhook reports error)
                   └──────────┘           └─────────┘
```

| Status | Trigger | Next State |
|--------|---------|------------|
| `pending` | Initial creation | → `pending_submission` (after compliance check) |
| `pending_submission` | Compliance approved, queued for worker | → `processing` |
| `processing` | Worker claimed task | → `submitted` (success) or retry (failure) |
| `submitted` | Transaction propagated to Solana | → `confirmed` (via webhook) |
| `confirmed` | Helius webhook confirms finalization | Terminal state |
| `failed` | Max retries (10) exceeded | Terminal state |

---

## Getting Started

### Prerequisites

- Rust 1.85+ (2024 edition)
- Node.js 18+ (for frontend)
- Docker & Docker Compose
- PostgreSQL 16+
- [sqlx-cli](https://github.com/launchbadge/sqlx) for migrations

### Quick Start

```bash
# Clone backend and frontend (frontend is a separate repo)
git clone https://github.com/berektassuly/solana-compliance-relayer.git
git clone https://github.com/Berektassuly/solana-compliance-relayer-frontend.git
cd solana-compliance-relayer

# Start PostgreSQL
docker-compose up -d

# Run database migrations
cargo sqlx migrate run

# Start the backend
cargo run
```

The backend runs on `http://localhost:3000`.

In another terminal, run the frontend:

```bash
cd ../solana-compliance-relayer-frontend
pnpm install
pnpm run dev
```

The frontend runs on `http://localhost:3001`.

---

## Environment Configuration

Create a `.env` file in the project root. See [`.env.example`](.env.example) for all options.

### Critical Variables

| Variable | Required | Description |
|----------|----------|-------------|
| `DATABASE_URL` | Yes | PostgreSQL connection string |
| `SOLANA_RPC_URL` | Yes | Solana RPC endpoint (Helius/QuickNode recommended) |
| `ISSUER_PRIVATE_KEY` | Yes | Base58 relayer wallet private key |
| `HELIUS_WEBHOOK_SECRET` | Recommended | Authorization header for Helius webhook validation |
| `RANGE_API_KEY` | No | Range Protocol API key (mock mode if absent) |
| `RANGE_API_URL` | No | Override Range API base URL (default: `https://api.range.org/v1`) |
| `RANGE_RISK_THRESHOLD` | No | Risk score threshold 1–10 (default: 6 = High Risk); ≥ threshold = reject |

### Optional Variables

| Variable | Description |
|----------|-------------|
| `HOST` | Bind interface (default: `0.0.0.0`) |
| `PORT` | Server port (default: `3000`) |
| `ENABLE_RATE_LIMITING` | Governor rate limiting (default: `false`) |
| `RATE_LIMIT_RPS` | Requests per second (default: `10`) |
| `RATE_LIMIT_BURST` | Burst size (default: `20`) |
| `ENABLE_BACKGROUND_WORKER` | Retry worker for pending submissions (default: `true`) |
| `ENABLE_PRIVACY_CHECKS` | QuickNode Privacy Health Check for confidential transfers (default: `true`) |
| `CORS_ALLOWED_ORIGINS` | Comma-separated CORS origins; `localhost:3000` and `localhost:3001` always allowed |
| `RUST_LOG` | Log level (e.g. `info,tower_http=debug,sqlx=warn`) |

### RPC Provider Variables

| Variable | Required | Description |
|----------|----------|-------------|
| `SOLANA_RPC_URL` | Yes | Solana RPC endpoint. Supports **Helius** (`helius-rpc.com`), **QuickNode** (`quiknode.pro`), or standard RPC. Provider-specific features (priority fees, DAS, Jito) are auto-detected from the URL. |

### Jito MEV Protection Variables (QuickNode only)

| Variable | Default | Description |
|----------|---------|-------------|
| `USE_JITO_BUNDLES` | `false` | Enable "Ghost Mode" - private transaction submission via Jito block builders for MEV protection |
| `JITO_TIP_LAMPORTS` | `10000` | Tip amount in lamports for Jito block builders (0.00001 SOL). Higher tips increase inclusion priority. Recommended: 10,000-50,000 |
| `JITO_REGION` | auto | Optional region for lower latency: `ny`, `amsterdam`, `frankfurt`, `tokyo` |

### Webhook Variables

| Variable | Description |
|----------|-------------|
| `HELIUS_WEBHOOK_SECRET` | Authorization header value for validating Helius webhook requests |
| `QUICKNODE_WEBHOOK_SECRET` | Authorization header value for validating QuickNode Streams (webhook) requests |

> **Note:** Priority fees and DAS compliance are auto-detected from `SOLANA_RPC_URL` (Helius/QuickNode). Jito bundles require QuickNode with the "Lil' JIT" add-on enabled.

### Example Production Configuration (Helius)

```env
# Database
DATABASE_URL=postgres://user:pass@host:5432/compliance_relayer

# Blockchain (Helius - recommended for webhooks)
SOLANA_RPC_URL=https://mainnet.helius-rpc.com/?api-key=YOUR_HELIUS_KEY
ISSUER_PRIVATE_KEY=YOUR_BASE58_PRIVATE_KEY
HELIUS_WEBHOOK_SECRET=YOUR_WEBHOOK_SECRET

# Compliance
RANGE_API_KEY=YOUR_RANGE_KEY
RANGE_RISK_THRESHOLD=6

# Server
HOST=0.0.0.0
PORT=3000

# Features
ENABLE_RATE_LIMITING=true
ENABLE_BACKGROUND_WORKER=true
ENABLE_PRIVACY_CHECKS=true

# Rate limiting
RATE_LIMIT_RPS=10
RATE_LIMIT_BURST=20

# CORS
CORS_ALLOWED_ORIGINS=https://your-frontend.example.com
```

### Example Production Configuration (QuickNode + Jito MEV Protection)

```env
# Database
DATABASE_URL=postgres://user:pass@host:5432/compliance_relayer

# Blockchain (QuickNode - recommended for MEV protection)
SOLANA_RPC_URL=https://your-endpoint.solana-mainnet.quiknode.pro/YOUR_API_KEY
ISSUER_PRIVATE_KEY=YOUR_BASE58_PRIVATE_KEY
QUICKNODE_WEBHOOK_SECRET=YOUR_WEBHOOK_SECRET

# Jito MEV Protection (requires QuickNode "Lil' JIT" add-on)
USE_JITO_BUNDLES=true
JITO_TIP_LAMPORTS=10000
JITO_REGION=ny

# Compliance
RANGE_API_KEY=YOUR_RANGE_KEY
RANGE_RISK_THRESHOLD=6

# Server
HOST=0.0.0.0
PORT=3000

# Features
ENABLE_RATE_LIMITING=true
ENABLE_BACKGROUND_WORKER=true
ENABLE_PRIVACY_CHECKS=true

# Rate limiting
RATE_LIMIT_RPS=10
RATE_LIMIT_BURST=20

# CORS
CORS_ALLOWED_ORIGINS=https://your-frontend.example.com
```

---

## API Reference

### Core Endpoints

| Method | Path | Description |
|--------|------|-------------|
| `POST` | `/transfer-requests` | Submit a signed transfer request |
| `GET` | `/transfer-requests` | List transfers (paginated) |
| `GET` | `/transfer-requests/{id}` | Get transfer by ID |
| `POST` | `/transfer-requests/{id}/retry` | Retry failed submission |
| `POST` | `/webhooks/helius` | Helius webhook receiver |
| `GET` | `/health` | Detailed health check |
| `GET` | `/health/live` | Kubernetes liveness probe |
| `GET` | `/health/ready` | Kubernetes readiness probe |

### Admin Endpoints (Blocklist Management)

| Method | Path | Description |
|--------|------|-------------|
| `POST` | `/admin/blocklist` | Add address to internal blocklist. Body: `{"address": "...", "reason": "..."}` |
| `GET` | `/admin/blocklist` | List all blocklisted addresses |
| `DELETE` | `/admin/blocklist/{address}` | Remove address from blocklist |

### Compliance Endpoints

| Method | Path | Description |
|--------|------|-------------|
| `POST` | `/risk-check` | Pre-flight wallet risk check (aggregates blocklist, Range, Helius DAS). Request body: `{"address": "<wallet_pubkey>"}`. Results cached 1 hour. |

### Interactive Documentation

- **Swagger UI:** `http://localhost:3000/swagger-ui`
- **OpenAPI Spec:** `http://localhost:3000/api-docs/openapi.json`

### Request Uniqueness (Nonce & Idempotency)

**Breaking change (v2.0):** Every `POST /transfer-requests` request must include a **nonce** in the body, and the **signature must be computed over a message that includes the nonce**. This prevents replay attacks and enables idempotent retries.

- **Nonce:** Required. A unique value per request (e.g. UUID v4 or v7). Must be 32–64 characters, alphanumeric with optional hyphens.
- **Idempotency-Key header:** Optional but **recommended**. If sent, it must equal the body `nonce`. Duplicate requests with the same nonce return the existing transfer (200) instead of creating a new one.

### Signing Message Format (Critical for Integration)

**The message you sign must use this exact format.** Using the old format will cause signature verification to fail.

| Version | Format |
|--------|--------|
| **Old (deprecated)** | `{from}:{to}:{amount}:{mint}` |
| **Current (required)** | **`{from}:{to}:{amount}:{mint}:{nonce}`** |

- For **public** transfers: `amount` is the numeric amount (e.g. `1000000000`); for **confidential** use the literal `confidential`.
- For **SOL**: use `SOL` for `mint`; for SPL tokens use the mint address.
- **`nonce`** must be the same value sent in the JSON body (e.g. a UUID string).

Example message (public SOL):  
`7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU:RecipientPubkey...:1000000000:SOL:019470a4-7e7c-7d3e-8f1a-2b3c4d5e6f7a`

### Example: Submit Public Transfer

```bash
curl -X POST http://localhost:3000/transfer-requests \
  -H "Content-Type: application/json" \
  -H "Idempotency-Key: 019470a4-7e7c-7d3e-8f1a-2b3c4d5e6f7a" \
  -d '{
    "from_address": "YOUR_WALLET_PUBKEY",
    "to_address": "RECIPIENT_PUBKEY",
    "transfer_details": {
      "type": "public",
      "amount": 1000000000
    },
    "token_mint": null,
    "signature": "BASE58_ED25519_SIGNATURE_OVER_MESSAGE_WITH_NONCE",
    "nonce": "019470a4-7e7c-7d3e-8f1a-2b3c4d5e6f7a"
  }'
```

**Headers:** `Idempotency-Key` is supported and recommended; when present it must match the body `nonce`.

---

## Client Integration / SDK

When integrating with the API (WASM, mobile, or server SDKs):

- **Generate a unique nonce per request.** Use UUID v4 or (recommended) UUID v7 for time-ordered uniqueness. The nonce must be included in both the request body and the **signed message** (see [Signing Message Format](#signing-message-format-critical-for-integration)).
- **Send the same nonce as `Idempotency-Key`** when retrying the same logical request (e.g. after a timeout) so the server returns the original response instead of creating a duplicate transfer.

---

## CLI Tools

The project includes CLI utilities for generating valid transfer requests with proper Ed25519 signatures.

### generate_transfer_request

Generates a complete, signed transfer request and outputs a ready-to-use curl command. Uses a dev keypair (or override via code). **For production, use client-side signing.** The generated request includes a nonce and uses the current signing format (`{from}:{to}:{amount}:{mint}:{nonce}`).

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

Creates real on-chain confidential transfer state on the **zk-edge** testnet (`https://zk-edge.surfnet.dev:8899`), then generates valid ZK proofs and a `TransferRequest` JSON for the relayer. Use this for end-to-end testing of Token-2022 confidential transfers.

**Steps:** Create mint → source/dest token accounts → mint → deposit → apply pending balance → generate ZK proofs → output curl command.

```bash
cargo run --bin setup_and_generate
```

Requires airdrop-funded authority on zk-edge. Output includes mint, ATAs, and a ready-to-use `curl` for `POST /transfer-requests`.

---

## Testing

```bash
# Run all tests
cargo test

# Run with verbose output
cargo test -- --nocapture

# Run integration tests (requires PostgreSQL, e.g. docker-compose)
cargo test --test integration_test

# Database integration tests (single-threaded)
cargo test --test database_integration -- --test-threads=1

# API and infra tests
cargo test --test api_requests
cargo test --test infra_blockchain_http_tests
cargo test --test infra_compliance_tests

# Coverage (requires cargo-tarpaulin)
cargo tarpaulin --out Html
```

---

## Deployment

### Backend (Railway)

1. Connect repository to Railway
2. Add PostgreSQL service
3. Set environment variables (see [Environment Configuration](#environment-configuration))
4. Configure build command: `cargo build --release`
5. Configure start command: `./target/release/solana-compliance-relayer`

### Backend (Docker)

A [Dockerfile](Dockerfile) is provided. Build and run:

```bash
docker build -t solana-compliance-relayer .
docker run --env-file .env -p 3000:3000 solana-compliance-relayer
```

Ensure PostgreSQL is reachable (e.g. via `DATABASE_URL`). Use [docker-compose](docker-compose.yml) for local PostgreSQL.

### Frontend (Vercel)

1. Import the [frontend repository](https://github.com/Berektassuly/solana-compliance-relayer-frontend)
2. Configure environment variables for API URL
3. Deploy with default Next.js preset

### Helius Webhook Configuration

1. Go to Helius Dashboard → Webhooks
2. Create new webhook:
   - **URL:** `https://your-backend.railway.app/webhooks/helius`
   - **Type:** Enhanced Transactions
   - **Auth Header:** Your `HELIUS_WEBHOOK_SECRET` value
   - **Account Addresses:** Add your relayer wallet public key

---

## Roadmap

| Phase | Feature | Status |
|-------|---------|--------|
| 1 | Core relayer with Range Protocol | Complete |
| 2 | Background worker with exponential backoff | Complete |
| 3 | Rate limiting and observability | Complete |
| 4 | WASM client-side signing | Complete |
| 5 | Helius webhook integration | Complete |
| 6 | Next.js frontend with real-time monitoring | Complete |
| 7 | Token-2022 confidential transfer support | Complete |
| 8 | Internal Blocklist Manager with admin API | Complete |
| 9 | Pre-Flight Risk Check with persistent caching | Complete |
| 10 | `setup_and_generate` CLI for zk-edge confidential transfers | Complete |
| 11 | **Jito Bundle Integration (MEV Protection)** | Complete |
| 12 | **Smart Rent Recovery** (auto-close ZK context accounts) | Complete |
| 13 | **Double-Spend Protection** (status-aware retry for JitoStateUnknown) | Complete |
| 14 | **QuickNode Streams Integration** (dual-confirmation webhooks) | Complete |
| 15 | **Request Uniqueness** — Replay protection via nonces; API idempotency via Idempotency-Key | Complete |

---

## Contact

**Mukhammedali Berektassuly**

> This project was built with 💜 by a 17-year-old developer from Kazakhstan

- Website: [berektassuly.com](https://berektassuly.com)
- Email: [mukhammedali@berektassuly.com](mailto:mukhammedali@berektassuly.com)
- X/Twitter: [@berektassuly](https://x.com/berektassuly)

---

## License

This project is licensed under the MIT License. See [LICENSE](LICENSE) for details.

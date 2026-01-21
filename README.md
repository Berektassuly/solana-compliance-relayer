<div align="center">

# Solana Compliance Relayer

### Bridging the gap between on-chain privacy, regulatory compliance, and high-throughput execution.

[![Rust](https://img.shields.io/badge/Rust-000000?style=for-the-badge&logo=rust&logoColor=white)](https://www.rust-lang.org/)
[![PostgreSQL](https://img.shields.io/badge/PostgreSQL-4169E1?style=for-the-badge&logo=postgresql&logoColor=white)](https://www.postgresql.org/)
[![Solana](https://img.shields.io/badge/Solana-9945FF?style=for-the-badge&logo=solana&logoColor=white)](https://solana.com/)
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
- [Transaction Lifecycle](#transaction-lifecycle)
- [Getting Started](#getting-started)
- [Environment Configuration](#environment-configuration)
- [API Reference](#api-reference)
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

3. **Execution & Finalization (Helius)**: Approved transactions are submitted via Helius RPC with priority fee optimization. Webhooks provide real-time confirmation callbacks.

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
│  │  POST /webhooks/helius    │  GET /health                        │    │
│  └──────────────────────────────┬──────────────────────────────────┘    │
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
│   └── worker.rs    # Background retry worker with exponential backoff
├── api/             # HTTP interface (Primary Adapter)
│   ├── handlers.rs  # Axum route handlers with OpenAPI docs
│   ├── admin.rs     # Admin API for blocklist management
│   └── router.rs    # Rate limiting, CORS, middleware
└── infra/           # External integrations (Secondary Adapters)
    ├── database/    # PostgreSQL via SQLx (compile-time checked)
    ├── blockchain/  # Solana via Helius/QuickNode/Standard RPC
    ├── blocklist/   # Internal blocklist with DashMap + PostgreSQL
    └── compliance/  # Range Protocol integration
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
    WASM->>API: POST /transfer-requests (Signed Payload)
    
    API->>API: Verify Ed25519 Signature
    API->>API: Check Internal Blocklist (DashMap)
    
    alt Address in Internal Blocklist
        API->>DB: Persist (status: rejected, error: "Blocklist: reason")
        API-->>User: 200 OK {blockchain_status: "failed"}
    else Address Not Blocked
        API->>Range: check_compliance(address)
        alt Address High Risk (riskScore >= 6)
            Range-->>API: Rejected (CRITICAL/HIGH risk)
            API->>DB: Persist (compliance_status: rejected)
            API->>API: Auto-add to Internal Blocklist
            API-->>User: 200 OK {blockchain_status: "failed"}
        else Address Clean
            Range-->>API: Approved (riskScore < 6)
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
| **Real-Time Transaction Monitoring** | Frontend polls API every 5 seconds with TanStack Query |
| **Internal Blocklist Manager** | Thread-safe DashMap cache with PostgreSQL persistence for fast local address screening |
| **Automated AML/Compliance Screening** | Range Protocol Risk API with 1-10 score scale (>=6 = High risk = rejected) |
| **Public & Confidential Transfers** | Supports standard SOL/SPL and Token-2022 ZK confidential transfers |
| **Resilient Background Worker** | Exponential backoff retries (up to 10 attempts, max 5-minute delay) |
| **Helius Webhook Integration** | Real-time finalization callbacks move transactions from `submitted` -> `confirmed` |
| **Provider Strategy Pattern** | Auto-detects Helius/QuickNode for premium features (priority fees, DAS) |
| **Rate Limiting** | Governor-based middleware with configurable RPS and burst limits |
| **OpenAPI Documentation** | Auto-generated Swagger UI at `/swagger-ui` |

---

## Technical Stack

### Backend

| Component | Technology |
|-----------|------------|
| Language | Rust 1.75+ |
| Web Framework | Axum 0.8 |
| Database | PostgreSQL 16+ (SQLx with compile-time verification) |
| Async Runtime | Tokio |
| HTTP Client | Reqwest |
| Rate Limiting | Governor |
| API Docs | utoipa (OpenAPI 3.0) |
| Middleware | Tower-HTTP (tracing, timeout, CORS) |

### Frontend

| Component | Technology |
|-----------|------------|
| Framework | Next.js 14 (App Router) |
| Styling | Tailwind CSS |
| State Management | TanStack Query (React Query) |
| Build | Turbopack |

### Cryptography

| Component | Technology |
|-----------|------------|
| Signing | Ed25519-dalek (WASM-compiled) |
| ZK Proofs | solana-zk-sdk, spl-token-confidential-transfer-proof-generation |
| Key Derivation | ElGamal, AES-256 |

### Infrastructure

| Component | Technology |
|-----------|------------|
| RPC Provider | Helius / QuickNode (auto-detected) |
| Compliance | Range Protocol Risk API |
| Deployment (Backend) | Railway |
| Deployment (Frontend) | Vercel |
| Database Hosting | Railway PostgreSQL |

---

## RPC Provider Strategy

The relayer implements a **Provider Strategy Pattern** that auto-detects the RPC endpoint and activates premium features accordingly:

| Provider | Detection | Features |
|----------|-----------|----------|
| **Helius** | URL contains `helius-rpc.com` | Priority fee estimation via `getPriorityFeeEstimate`, DAS compliance checks, Enhanced Webhooks |
| **QuickNode** | URL contains `quiknode.pro` or `quicknode.com` | Priority fee estimation via `qn_estimatePriorityFees`, Privacy Health Check service, Ghost Mode (Jito bundles) |
| **Standard** | Any other RPC | Static fallback fee strategy (5000 micro-lamports) |

### QuickNode-Specific Features

- **Priority Fee Estimation**: Uses the `qn_estimatePriorityFees` RPC method to fetch real-time fee recommendations
- **Privacy Health Check Service**: Monitors token activity to recommend optimal submission timing for confidential transfers
- **Ghost Mode Integration**: Scaffolded for private transaction submission via Jito bundles

### Configuration Examples

```env
# Helius (recommended for webhooks)
SOLANA_RPC_URL=https://mainnet.helius-rpc.com/?api-key=YOUR_HELIUS_KEY

# QuickNode (recommended for privacy features)
SOLANA_RPC_URL=https://your-endpoint.solana-mainnet.quiknode.pro/YOUR_API_KEY

# Standard RPC (development only)
SOLANA_RPC_URL=https://api.devnet.solana.com
```

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
│    Instant reject           Risk score ≥70                          │
│    (no API call)            = Rejected                              │
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

- Rust 1.75+
- Node.js 18+ (for frontend)
- Docker & Docker Compose
- PostgreSQL 16+

### Quick Start

```bash
# Clone the repository
git clone https://github.com/berektassuly/solana-compliance-relayer.git
git clone https://github.com/Berektassuly/solana-compliance-relayer-frontend.git
cd solana-compliance-relayer

# Start PostgreSQL
docker-compose up -d

# Run database migrations
cargo sqlx migrate run

# Start the backend
cargo run

# In another terminal, start the frontend
cd frontend
pnpm install
pnpm run dev
```

The backend will start on `http://localhost:3000`.
The frontend will start on `http://localhost:3001`.

---

## Environment Configuration

Create a `.env` file in the project root. See `.env.example` for all options.

### Critical Variables

| Variable | Required | Description |
|----------|----------|-------------|
| `DATABASE_URL` | ✅ | PostgreSQL connection string |
| `SOLANA_RPC_URL` | ✅ | Solana RPC endpoint (Helius recommended) |
| `ISSUER_PRIVATE_KEY` | ✅ | Base58 relayer wallet private key |
| `HELIUS_API_KEY` | ⚡ | Enables priority fees and DAS checks (auto-detected from RPC URL) |
| `HELIUS_WEBHOOK_SECRET` | ⚡ | Authorization header for webhook validation |
| `RANGE_API_KEY` | ⚠️ | Range Protocol API key (mock mode if absent) |

> ⚡ = Highly recommended for production  
> ⚠️ = Falls back to mock mode if not set

### Example Production Configuration

```env
# Database
DATABASE_URL=postgres://user:pass@host:5432/compliance_relayer

# Blockchain (Helius)
SOLANA_RPC_URL=https://mainnet.helius-rpc.com/?api-key=YOUR_HELIUS_KEY
ISSUER_PRIVATE_KEY=YOUR_BASE58_PRIVATE_KEY
HELIUS_WEBHOOK_SECRET=YOUR_WEBHOOK_SECRET

# Compliance
RANGE_API_KEY=YOUR_RANGE_KEY

# Server
HOST=0.0.0.0
PORT=3000

# Features
ENABLE_RATE_LIMITING=true
ENABLE_BACKGROUND_WORKER=true
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
| `POST` | `/admin/blocklist` | Add address to internal blocklist |
| `GET` | `/admin/blocklist` | List all blocklisted addresses |
| `DELETE` | `/admin/blocklist/{address}` | Remove address from blocklist |

### Interactive Documentation

- **Swagger UI:** `http://localhost:3000/swagger-ui`
- **OpenAPI Spec:** `http://localhost:3000/api-docs/openapi.json`

### Example: Submit Public Transfer

```bash
curl -X POST http://localhost:3000/transfer-requests \
  -H "Content-Type: application/json" \
  -d '{
    "from_address": "YOUR_WALLET_PUBKEY",
    "to_address": "RECIPIENT_PUBKEY",
    "transfer_details": {
      "type": "public",
      "amount": 1000000000
    },
    "signature": "BASE58_ED25519_SIGNATURE"
  }'
```

---

## CLI Tools

The project includes CLI utilities for generating valid transfer requests with proper Ed25519 signatures.

### generate_transfer_request

Generates a complete, signed transfer request and outputs a ready-to-use curl command.

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

Signing Message: "7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU:randomDest...:1000000000:SOL"

Generated curl command:

curl -X POST 'http://localhost:3000/transfer-requests' \
  -H 'Content-Type: application/json' \
  -d '{
    "from_address": "7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU",
    "to_address": "randomDestination...",
    "transfer_details": {
      "type": "public",
      "amount": 1000000000
    },
    "signature": "BASE58_SIGNATURE..."
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

---

## Testing

```bash
# Run all tests
cargo test

# Run with verbose output
cargo test -- --nocapture

# Run integration tests (requires Docker)
cargo test --test integration_test

# Run with coverage
cargo tarpaulin --out Html
```

---

## Deployment

### Railway (Backend)

1. Connect repository to Railway
2. Add PostgreSQL service
3. Set environment variables
4. Configure build command: `cargo build --release`
5. Configure start command: `./target/release/solana-compliance-relayer`

### Vercel (Frontend)

1. Import frontend directory
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

# Configuration

This document covers environment configuration, RPC provider strategy, and deployment options for the Solana Compliance Relayer.

---

## Table of Contents

- [Environment Configuration](#environment-configuration)
- [RPC Provider Strategy](#rpc-provider-strategy)
- [Production Configuration Examples](#production-configuration-examples)
- [Deployment](#deployment)
- [Testing](#testing)

---

## Environment Configuration

Create a `.env` file in the project root. See [`.env.example`](../.env.example) for all options.

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

### Server Variables

| Variable | Default | Description |
|----------|---------|-------------|
| `HOST` | `0.0.0.0` | Bind interface |
| `PORT` | `3000` | Server port |
| `RUST_LOG` | `info` | Log level (e.g., `info,tower_http=debug,sqlx=warn`) |

### Feature Flags

| Variable | Default | Description |
|----------|---------|-------------|
| `ENABLE_RATE_LIMITING` | `false` | Governor rate limiting |
| `ENABLE_BACKGROUND_WORKER` | `true` | Retry worker for pending submissions |
| `ENABLE_PRIVACY_CHECKS` | `true` | QuickNode Privacy Health Check for confidential transfers |

### Rate Limiting Variables

| Variable | Default | Description |
|----------|---------|-------------|
| `RATE_LIMIT_RPS` | `10` | Requests per second |
| `RATE_LIMIT_BURST` | `20` | Burst size |

### CORS Configuration

| Variable | Default | Description |
|----------|---------|-------------|
| `CORS_ALLOWED_ORIGINS` | — | Comma-separated CORS origins; `localhost:3000` and `localhost:3001` always allowed |

### Jito MEV Protection Variables (QuickNode only)

| Variable | Default | Description |
|----------|---------|-------------|
| `USE_JITO_BUNDLES` | `false` | Enable "Ghost Mode" - private transaction submission via Jito block builders |
| `JITO_TIP_LAMPORTS` | `10000` | Tip amount in lamports (0.00001 SOL). Recommended: 10,000-50,000 |
| `JITO_REGION` | auto | Optional region for lower latency: `ny`, `amsterdam`, `frankfurt`, `tokyo` |

### Webhook Variables

| Variable | Description |
|----------|-------------|
| `HELIUS_WEBHOOK_SECRET` | Authorization header value for validating Helius webhook requests |
| `QUICKNODE_WEBHOOK_SECRET` | Authorization header value for validating QuickNode Streams requests |

---

## RPC Provider Strategy

The relayer implements a **Provider Strategy Pattern** that auto-detects the RPC endpoint and activates premium features accordingly.

### Provider Detection

| Provider | Detection | Features Enabled |
|----------|-----------|------------------|
| **Helius** | URL contains `helius-rpc.com` | Priority fee estimation via `getPriorityFeeEstimate`, DAS compliance checks, Enhanced Webhooks |
| **QuickNode** | URL contains `quiknode.pro` or `quicknode.com` | Priority fee estimation via `qn_estimatePriorityFees`, Privacy Health Check service, **Jito Bundle Submission (MEV Protection)** |
| **Standard** | Any other RPC | Static fallback fee strategy (5000 micro-lamports) |

### QuickNode-Specific Features

- **Priority Fee Estimation:** Uses the `qn_estimatePriorityFees` RPC method to fetch real-time fee recommendations
- **Privacy Health Check Service:** Monitors token activity to recommend optimal submission timing for confidential transfers
- **Jito Bundle Submission (Ghost Mode):** MEV-protected private transaction submission via Jito block builders

### Configuration Examples

```env
# Helius (recommended for webhooks)
SOLANA_RPC_URL=https://mainnet.helius-rpc.com/?api-key=YOUR_HELIUS_KEY

# QuickNode (recommended for MEV protection + privacy features)
SOLANA_RPC_URL=https://your-endpoint.solana-mainnet.quiknode.pro/YOUR_API_KEY

# Standard RPC (development only)
SOLANA_RPC_URL=https://api.devnet.solana.com
```

> **Note:** Priority fees and DAS compliance are auto-detected from `SOLANA_RPC_URL`. Jito bundles require QuickNode with the "Lil' JIT" add-on enabled.

---

## Production Configuration Examples

### Helius Configuration

Recommended for projects prioritizing webhook reliability and DAS integration.

```env
# Database
DATABASE_URL=postgres://user:pass@host:5432/compliance_relayer

# Blockchain (Helius)
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

### QuickNode + Jito MEV Protection Configuration

Recommended for projects requiring MEV protection and confidential transfer support.

```env
# Database
DATABASE_URL=postgres://user:pass@host:5432/compliance_relayer

# Blockchain (QuickNode)
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

## Deployment

### Prerequisites

- Rust 1.85+ (2024 edition)
- Node.js 18+ (for frontend)
- Docker & Docker Compose
- PostgreSQL 16+
- [sqlx-cli](https://github.com/launchbadge/sqlx) for migrations

### Quick Start (Local Development)

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

### Backend (Railway)

1. Connect repository to Railway
2. Add PostgreSQL service
3. Set environment variables (see [Environment Configuration](#environment-configuration))
4. Configure build command: `cargo build --release`
5. Configure start command: `./target/release/solana-compliance-relayer`

### Backend (Docker)

A [Dockerfile](../Dockerfile) is provided. Build and run:

```bash
docker build -t solana-compliance-relayer .
docker run --env-file .env -p 3000:3000 solana-compliance-relayer
```

Ensure PostgreSQL is reachable (e.g., via `DATABASE_URL`). Use [docker-compose](../docker-compose.yml) for local PostgreSQL.

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

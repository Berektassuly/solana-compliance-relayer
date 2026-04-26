<div align="center">

# Solana Compliance Relayer

### Compliance-first Solana payment infrastructure for stablecoin checkout, remittance, and virtual-card funding.

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

## Product Focus

The relayer lets fintech applications accept or send Solana stablecoin payments while enforcing compliance before settlement. Clean transfers are submitted privately and reliably. Risky transfers are rejected before chain submission but retained for audit.

Built for S1lkPay / Frontier-style payment products, it exposes a backend API for merchant checkout sessions, remittance flows, virtual card funding, internal blocklist operations, Range Protocol AML screening, Helius/QuickNode confirmation webhooks, and audit reports merchants can store for compliance review.

---

## S1lkPay / Frontier Demo Flow

1. A merchant, wallet, or card program creates a checkout session for a USDC payment using `POST /checkout/sessions`.
2. The customer signs a Solana transfer payload in their wallet. The signed payload includes amount, mint, recipient, and nonce.
3. The app submits the signed transfer to `POST /checkout/sessions/{id}/submit-transfer`.
4. The relayer verifies the signature, enforces nonce replay protection, checks the internal blocklist, screens the recipient with Range Protocol, and persists the decision.
5. Approved transfers are queued for private/reliable Solana submission. Rejected transfers never settle on-chain.
6. The merchant fetches `GET /transfer-requests/{id}/audit-report` to see the final compliance and settlement decision.

### Merchant Checkout Example

```bash
curl -X POST http://localhost:3000/checkout/sessions \
  -H "Content-Type: application/json" \
  -d '{
    "merchant_id": "merchant_kz_001",
    "merchant_reference": "INV-2026-00042",
    "destination_wallet": "DRpbCBMxVnDK7maPM5tGv6MvB3v1sRMC86PZ8okm21hy",
    "token_mint": "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v",
    "amount": 25000000,
    "customer_wallet": "7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU",
    "merchant_metadata": {
      "purpose": "virtual_card_funding",
      "currency": "USDC"
    }
  }'
```

### Audit Report Example

```json
{
  "transfer_id": "550e8400-e29b-41d4-a716-446655440000",
  "asset_type": "spl_token",
  "token_mint": "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v",
  "amount": { "visibility": "public", "amount": 25000000 },
  "compliance_status": "approved",
  "blockchain_status": "pending_submission",
  "risk_decision_summary": "Approved by compliance controls and queued or submitted for settlement.",
  "final_decision": "approved_for_settlement"
}
```

## Live Demo

[Watch the demo on YouTube](https://youtu.be/LSMlIqtrxL0) — dashboard, risk scanning, Jito MEV transaction, and Range Protocol compliance blocking.

---

## Why This Exists

Stablecoin payment providers need instant settlement, low fees, and programmable controls, but fintech operators also need compliance evidence before they move money. The Solana Compliance Relayer resolves this with a defense-in-depth payment pipeline for merchant checkout, cross-border remittance, and card-funding flows.

| Challenge | Solution |
|-----------|----------|
| Blinded signing risk | Client-side WASM signing ensures wallets never expose private keys to the server |
| Regulatory compliance | Real-time AML/Sanctions screening via Range Protocol before chain submission |
| Transaction guarantees | Transactional Outbox pattern with PostgreSQL ensures no approved tx is ever lost |
| MEV extraction | Private submission via Jito Bundles bypasses the public mempool |

> **Core Guarantee:** Rejected transactions are persisted for audit but **never** submitted to the blockchain.

---

## Key Features

| Feature | Description |
|---------|-------------|
| **MEV Protection (Ghost Mode)** | Private transaction submission via Jito block builders—no frontrunning, no sandwich attacks |
| **Real-Time Compliance** | Automated AML/Sanctions screening via Range Protocol with configurable risk thresholds |
| **Merchant Checkout Sessions** | Durable sessions for stablecoin checkout, remittance, and virtual-card funding flows |
| **Compliance Audit Reports** | Machine-readable settlement and risk decision reports for merchants and operators |
| **Authenticated Admin Controls** | `/admin/*` routes support `ADMIN_API_KEY` for production blocklist management |
| **Client-Side WASM Signing** | Ed25519 via `ed25519-dalek` compiled to WebAssembly—private keys never leave the browser |
| **Replay Attack Protection** | Cryptographic enforcement of request uniqueness via nonces |
| **Double-Spend Protection** | Status-aware retry logic prevents duplicate submissions during network failures |
| **Confidential Transfers** | Full Token-2022 ZK confidential transfer support with automated rent recovery |

---

## Architecture

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
│                                │                                        │
│  ┌─────────────────────────────▼───────────────────────────────────┐    │
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

---

## Frontend

The official dashboard and UI for this relayer is a separate repository:

- **Repository:** [solana-compliance-relayer-frontend](https://github.com/Berektassuly/solana-compliance-relayer-frontend)
- **Live demo:** [solana-compliance-relayer-frontend.berektassuly.com/](https://solana-compliance-relayer-frontend.berektassuly.com/)

It provides a real-time dashboard (analytics, metrics, terminal, monitor), client-side WASM signing, risk scanner, and admin blocklist management. Built with Next.js, React, Tailwind CSS, and a Rust/WASM signing module.

---

## Quick Start

### Prerequisites

- Rust 1.85+ (2024 edition)
- Docker & Docker Compose
- PostgreSQL 16+

### Run with Docker

```bash
# Clone the repository
git clone https://github.com/berektassuly/solana-compliance-relayer.git
cd solana-compliance-relayer

# Start PostgreSQL
docker-compose up -d

# Run database migrations
cargo sqlx migrate run

# Start the backend
cargo run
```

The backend runs on `http://localhost:3000`. Swagger UI is available at `/swagger-ui`.

---

## Tech Stack

- **Backend:** Rust 1.85+, Axum 0.8, SQLx 0.8, Tokio 1.48
- **Frontend:** [solana-compliance-relayer-frontend](https://github.com/Berektassuly/solana-compliance-relayer-frontend) — Next.js, Tailwind CSS, Zustand, Rust/WASM signing
- **Cryptography:** ed25519-dalek (WASM), solana-zk-sdk, ElGamal/AES
- **RPC Providers:** Helius, QuickNode (auto-detected)
- **Compliance:** Range Protocol Risk API
- **MEV Protection:** Jito Bundles via QuickNode

---

## Documentation

| Document | Description |
|----------|-------------|
| [Architecture](docs/ARCHITECTURE.md) | Hexagonal architecture, directory structure, and data flow diagrams |
| [Submission Brief](SUBMISSION.md) | S1lkPay / Frontier problem, product, architecture, demo script, and roadmap |
| [Security](docs/SECURITY.md) | MEV protection, double-spend prevention, replay protection, blocklist manager |
| [API Reference](docs/API_REFERENCE.md) | Endpoints, signing message format, and example requests |
| [Configuration](docs/CONFIGURATION.md) | Environment variables, RPC provider strategy, deployment |
| [Client Integration](docs/CLIENT_INTEGRATION.md) | SDK integration notes and CLI tools |
| [Technical Operations Guide](docs/OPERATIONS.md) | Infrastructure config, database ops, and troubleshooting |
| [Contributing](CONTRIBUTING.md) | Contribution guidelines |

---

## Roadmap

| Phase | Feature | Status |
|-------|---------|--------|
| 1–6 | Core relayer, worker, WASM signing, webhooks, frontend | ✅ Complete |
| 7–10 | Confidential transfers, blocklist, risk checks, CLI tools | ✅ Complete |
| 11–15 | Jito MEV, rent recovery, double-spend protection, nonces | ✅ Complete |

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

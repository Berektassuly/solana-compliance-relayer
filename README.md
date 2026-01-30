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

## Why This Exists

Privacy-preserving protocols on Solana face a fundamental paradox: **users demand confidentiality**, but **institutions require auditability**. The Solana Compliance Relayer resolves this tension through a **Defense-in-Depth** architecture that ensures compliant, secure, and MEV-protected transactions.

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
- **Frontend:** Next.js 14, Tailwind CSS, TanStack Query
- **Cryptography:** ed25519-dalek (WASM), solana-zk-sdk, ElGamal/AES
- **RPC Providers:** Helius, QuickNode (auto-detected)
- **Compliance:** Range Protocol Risk API
- **MEV Protection:** Jito Bundles via QuickNode

---

## Documentation

| Document | Description |
|----------|-------------|
| [Architecture](docs/ARCHITECTURE.md) | Hexagonal architecture, directory structure, and data flow diagrams |
| [Security](docs/SECURITY.md) | MEV protection, double-spend prevention, replay protection, blocklist manager |
| [API Reference](docs/API_REFERENCE.md) | Endpoints, signing message format, and example requests |
| [Configuration](docs/CONFIGURATION.md) | Environment variables, RPC provider strategy, deployment |
| [Client Integration](docs/CLIENT_INTEGRATION.md) | SDK integration notes and CLI tools |
| [Guide](docs/GUIDE.md) | Step-by-step usage guide |
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

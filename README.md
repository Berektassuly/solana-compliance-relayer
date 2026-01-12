<div align="center">

# Solana Compliance Relayer

[![Rust](https://img.shields.io/badge/Rust-000000?style=for-the-badge&logo=rust&logoColor=white)](https://www.rust-lang.org/)
[![Solana](https://img.shields.io/badge/Solana-9945FF?style=for-the-badge&logo=solana&logoColor=white)](https://solana.com/)
[![License: MIT](https://img.shields.io/badge/License-MIT-22C55E?style=for-the-badge)](LICENSE)
[![Track](https://img.shields.io/badge/Track-Privacy%20Tooling-3B82F6?style=for-the-badge)](https://www.rangeprotocol.com/)
[![Author](https://img.shields.io/badge/Author-Berektassuly.com-F97316?style=for-the-badge)](https://berektassuly.com)

**A transactional gateway that screens wallet addresses via Range Protocol before relaying transactions to Solana, bridging privacy-preserving protocols with regulatory compliance.**

</div>

---

## Table of Contents

- [Overview](#overview)
- [Architecture](#architecture)
- [Data Flow](#data-flow)
- [Key Features](#key-features)
- [Tech Stack](#tech-stack)
- [Getting Started](#getting-started)
- [API Reference](#api-reference)
- [Guide](docs/GUIDE.md)
- [Configuration](#configuration)
- [Testing](#testing)
- [Roadmap](#roadmap)
- [Contact](#contact)
- [License](#license)

---

## Overview

Privacy-preserving protocols face a fundamental tension between user confidentiality and regulatory compliance. The Solana Compliance Relayer implements a **Defense-in-Depth** architecture that decouples compliance screening from transaction execution, enabling institutions to adopt privacy technologies while satisfying AML/KYC/OFAC requirements.

| Layer | Responsibility |
|-------|----------------|
| API Gateway | Request validation, rate limiting |
| Compliance Port | Address screening via Range Protocol |
| Persistence Layer | Audit trail with full status history |
| Blockchain Port | Transaction relay via Helius RPC |

**Key Guarantee:** Rejected transactions are persisted for audit purposes but are never submitted to the blockchain.

---

## Architecture

This project implements **Hexagonal Architecture** (Ports and Adapters), ensuring clean separation between business logic and infrastructure concerns.

```
src/
├── domain/          # Core business types and trait definitions (Ports)
│   ├── types.rs     # TransferRequest, ComplianceStatus, BlockchainStatus
│   ├── traits.rs    # DatabaseClient, BlockchainClient, ComplianceProvider
│   └── error.rs     # Unified error types
├── app/             # Application layer (Use Cases)
│   ├── service.rs   # Business logic orchestration
│   └── worker.rs    # Background retry worker
├── api/             # HTTP interface (Primary Adapter)
│   ├── handlers.rs  # Axum route handlers
│   └── router.rs    # OpenAPI-documented routes
└── infra/           # External integrations (Secondary Adapters)
    ├── database/    # PostgreSQL via SQLx
    ├── blockchain/  # Solana via Helius RPC
    └── compliance/  # Range Protocol integration
```

### Transactional Outbox Pattern

The system implements the **Transactional Outbox** pattern for guaranteed delivery:

1. **API Layer** validates and persists requests to PostgreSQL atomically
2. **Background Worker** polls for approved-but-unsubmitted transactions
3. **Blockchain Adapter** submits transactions with exponential backoff retry
4. **Status Updates** are persisted after each state transition

This pattern ensures no approved transaction is lost, even during service restarts or network failures.

---

## Data Flow

```mermaid
sequenceDiagram
    participant Client
    participant API as API Layer (Axum)
    participant Range as Range Protocol
    participant DB as PostgreSQL
    participant Worker as Background Worker
    participant Helius as Helius RPC
    participant Solana as Solana Network

    Client->>API: POST /transfer-requests
    API->>API: Validate request
    API->>Range: check_compliance(address)
    
    alt Address Sanctioned
        Range-->>API: Rejected
        API->>DB: Persist (compliance_status: rejected)
        API-->>Client: 200 OK (rejected)
    else Address Clean
        Range-->>API: Approved
        API->>DB: Persist (compliance_status: approved)
        API->>Helius: transfer_sol()
        Helius->>Solana: Submit transaction
        Solana-->>Helius: Signature
        Helius-->>API: tx_signature
        API->>DB: Update (blockchain_status: submitted)
        API-->>Client: 200 OK (submitted)
    end

    loop Every 10 seconds
        Worker->>DB: Poll pending + approved
        Worker->>Helius: Retry submissions
        Helius->>Solana: Submit transaction
        Worker->>DB: Update status
    end
```

---

## Key Features

| Feature | Description |
|---------|-------------|
| Real-time AML Screening | Pluggable compliance provider with Range Protocol adapter |
| Reliable Transaction Delivery | Transactional outbox with Helius RPC integration |
| Idempotency | UUID-based request tracking prevents duplicate submissions |
| Exponential Backoff | Up to 10 retries with configurable backoff strategy |
| Rate Limiting | Configurable limits via Governor middleware |
| Graceful Shutdown | SIGTERM/SIGINT handling with in-flight request completion |
| OpenAPI Documentation | Auto-generated Swagger UI at `/swagger-ui` |
| Health Monitoring | Database and blockchain connectivity checks |

---

## Tech Stack

| Component | Technology |
|-----------|------------|
| Language | Rust 1.75+ |
| Web Framework | Axum 0.8 |
| Database | PostgreSQL 16+ (SQLx) |
| Async Runtime | Tokio |
| HTTP Client | Reqwest |
| Cryptography | ed25519-dalek |
| Serialization | Serde |
| Validation | Validator |
| Documentation | utoipa (OpenAPI 3.0) |

---

## Getting Started

### Prerequisites

- Rust 1.75 or later
- Docker and Docker Compose
- PostgreSQL 16+ (provided via Docker)

### Environment Variables

Create a `.env` file in the project root:

```env
# Required
DATABASE_URL=postgres://postgres:postgres@localhost:5432/compliance_relayer

# Blockchain Configuration
SOLANA_RPC_URL=https://api.devnet.solana.com
ISSUER_PRIVATE_KEY=YOUR_BASE58_ENCODED_PRIVATE_KEY_HERE

# Server Configuration
HOST=0.0.0.0
PORT=3000

# Feature Flags
ENABLE_RATE_LIMITING=false
ENABLE_BACKGROUND_WORKER=true
```

### Quick Start

```bash
# Clone the repository
git clone https://github.com/berektassuly/solana-compliance-relayer.git
cd solana-compliance-relayer

# Start PostgreSQL
docker-compose up -d

# Run the application
cargo run
```

The server will start on `http://localhost:3000`.

---

## API Reference

### Endpoints

| Method | Path | Description |
|--------|------|-------------|
| POST | `/transfer-requests` | Submit a new transfer request |
| GET | `/transfer-requests` | List all transfers (paginated) |
| GET | `/transfer-requests/{id}` | Get transfer by ID |
| GET | `/health` | Health check |

### Interactive Documentation

- **Swagger UI:** `http://localhost:3000/swagger-ui`
- **OpenAPI Spec:** `http://localhost:3000/api-docs/openapi.json`

### Example Request

```bash
curl -X POST http://localhost:3000/transfer-requests \
  -H "Content-Type: application/json" \
  -d '{
    "from_address": "HvwC9QSAzwEXkUkwqNNGhfNHoVqXJYfPvPZfQvJmHWcF",
    "to_address": "DRpbCBMxVnDK7maPM5tGv6MvB3v1sRMC86PZ8okm21hy",
    "amount_sol": 0.5
  }'
```

### Response Schema

```json
{
  "id": "550e8400-e29b-41d4-a716-446655440000",
  "from_address": "HvwC9QSAzwEXkUkwqNNGhfNHoVqXJYfPvPZfQvJmHWcF",
  "to_address": "DRpbCBMxVnDK7maPM5tGv6MvB3v1sRMC86PZ8okm21hy",
  "amount_sol": 0.5,
  "compliance_status": "approved",
  "blockchain_status": "submitted",
  "blockchain_signature": "5eykt4UsFv8P8NJdTREpY1vzqKqZKvdp...",
  "blockchain_retry_count": 0,
  "created_at": "2026-01-12T16:00:00Z",
  "updated_at": "2026-01-12T16:00:00Z"
}
```

---

## Documentation

For a step-by-step testing walkthrough with curl examples, see the [Judge's Testing Guide](docs/GUIDE.md).

---

## Configuration

### Compliance Status Values

| Status | Description |
|--------|-------------|
| `pending` | Awaiting compliance check |
| `approved` | Passed sanctions screening |
| `rejected` | Failed sanctions screening |

### Blockchain Status Values

| Status | Description |
|--------|-------------|
| `pending` | Not yet processed |
| `pending_submission` | Queued for blockchain submission |
| `submitted` | Transaction sent to network |
| `confirmed` | Transaction confirmed on-chain |
| `failed` | Max retries exceeded |

---

## Testing

```bash
# Run all tests
cargo test

# Run with coverage
cargo tarpaulin --out Html

# Run integration tests (requires Docker)
cargo test --test integration_test
cargo test --test database_integration
```

---

## Roadmap

| Phase | Feature | Status |
|-------|---------|--------|
| 1 | Core relayer with Range Protocol | Complete |
| 2 | Background worker with exponential backoff | Complete |
| 3 | Rate limiting and observability | Complete |
| 4 | Zero-Knowledge proof support (Light Protocol) | Planned |
| 5 | Multi-signature governance | Planned |
| 6 | MEV protection via Jito bundles | Planned |

---

## Contact

**Mukhammedali Berektassuly**

- Website: [berektassuly.com](https://berektassuly.com)
- Email: [mukhammedali@berektassuly.com](mailto:mukhammedali@berektassuly.com)
- X/Twitter: [@berektassuly](https://x.com/berektassuly)

---

## License

This project is licensed under the MIT License. See [LICENSE](LICENSE) for details.

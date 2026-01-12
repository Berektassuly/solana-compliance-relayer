# Solana Compliance Relayer - Judge's Testing Guide

## Introduction

The Solana Compliance Relayer is a service that intercepts cryptocurrency transfer requests, screens them against a compliance provider, and only relays approved transactions to the Solana blockchain.

**Key Features:**

- **Compliance Screening**: Every transfer request is validated against a sanctions/blocklist before execution
- **Guaranteed Delivery**: Approved transactions are queued and retried with exponential backoff until confirmed on-chain
- **Audit Trail**: All requests (approved and rejected) are persisted with full status history

---

## Prerequisites

Before testing, ensure you have the following installed:

- **Docker** (for PostgreSQL database)
- **Rust/Cargo** (for running the application)

### Starting the Application

```bash
# 1. Start the PostgreSQL database
docker-compose up -d

# 2. Run the application
cargo run
```

The server will start on `http://localhost:3000`.

You can also access the interactive API documentation at `http://localhost:3000/swagger-ui`.

---

## Interactive Demo

The following scenarios demonstrate the core compliance functionality. Copy and paste these commands into your terminal.

### Scenario 1: The Sanctioned Wallet (Compliance Block)

This request attempts to transfer SOL to a wallet address that is flagged by the compliance provider. The address prefix "hack" triggers the mock sanctions list.

**Request:**

```bash
curl -X 'POST' \
  'http://localhost:3000/transfer-requests' \
  -H 'accept: application/json' \
  -H 'Content-Type: application/json' \
  -d '{
    "from_address": "HvwC9QSAzwEXkUkwqNNGhfNHoVqXJYfPvPZfQvJmHWcF",
    "to_address": "hackThePlanetBadWallet123456789012345678901",
    "amount_sol": 100.0
}'
```

**Expected Response:**

```json
{
  "id": "df752816-2fb8-4780-991b-d0f4c39c10f2",
  "from_address": "HvwC9QSAzwEXkUkwqNNGhfNHoVqXJYfPvPZfQvJmHWcF",
  "to_address": "hackThePlanetBadWallet123456789012345678901",
  "amount_sol": 100,
  "compliance_status": "rejected",
  "blockchain_status": "pending",
  "blockchain_signature": null,
  "blockchain_retry_count": 0,
  "blockchain_last_error": null,
  "blockchain_next_retry_at": null,
  "created_at": "2026-01-12T16:09:05.269006400Z",
  "updated_at": "2026-01-12T16:09:05.269006400Z"
}
```

**Key Observation:** The `compliance_status` is `"rejected"`. This transaction will never be submitted to the Solana blockchain.

---

### Scenario 2: The Clean Wallet (Successful Relay)

This request transfers SOL to a legitimate wallet address that passes compliance screening.

**Request:**

```bash
curl -X 'POST' \
  'http://localhost:3000/transfer-requests' \
  -H 'accept: application/json' \
  -H 'Content-Type: application/json' \
  -d '{
    "from_address": "HvwC9QSAzwEXkUkwqNNGhfNHoVqXJYfPvPZfQvJmHWcF",
    "to_address": "DRpbCBMxVnDK7maPM5tGv6MvB3v1sRMC86PZ8okm21hy",
    "amount_sol": 0.5
}'
```

**Expected Response:**

```json
{
  "id": "26bf6158-56d9-4442-a837-aad2ed0b98f4",
  "from_address": "HvwC9QSAzwEXkUkwqNNGhfNHoVqXJYfPvPZfQvJmHWcF",
  "to_address": "DRpbCBMxVnDK7maPM5tGv6MvB3v1sRMC86PZ8okm21hy",
  "amount_sol": 0.5,
  "compliance_status": "approved",
  "blockchain_status": "submitted",
  "blockchain_signature": "tx_4oGyFZFFQcuJvxhE",
  "blockchain_retry_count": 0,
  "blockchain_last_error": null,
  "blockchain_next_retry_at": null,
  "created_at": "2026-01-12T16:10:11.429116300Z",
  "updated_at": "2026-01-12T16:10:11.429116300Z"
}
```

**Key Observations:**

- The `compliance_status` is `"approved"` — the address passed sanctions screening
- The `blockchain_status` is `"submitted"` — the transaction was relayed to Solana
- The `blockchain_signature` contains the transaction hash

---

## Verification

### Viewing All Transfer Requests

To list all submitted transfer requests:

```bash
curl -X 'GET' 'http://localhost:3000/transfer-requests' -H 'accept: application/json'
```

### Checking Application Logs

The background worker processes approved transactions and logs its activity. When running with `cargo run`, observe the terminal output for messages such as:

```
INFO solana_compliance_relayer::app::worker > Processing 1 pending blockchain requests
INFO solana_compliance_relayer::app::worker > Successfully submitted transaction: tx_4oGyFZFFQcuJvxhE
```

These logs confirm that the worker is:

1. Polling for approved transactions with `pending_submission` status
2. Submitting them to the Solana blockchain
3. Updating the database with the resulting transaction signature

---

## API Reference

| Endpoint | Method | Description |
|----------|--------|-------------|
| `/transfer-requests` | POST | Submit a new transfer request |
| `/transfer-requests` | GET | List all transfer requests (paginated) |
| `/transfer-requests/{id}` | GET | Get a specific transfer request by ID |
| `/health` | GET | Health check endpoint |
| `/swagger-ui` | GET | Interactive API documentation |

---

## Architecture Summary

```
┌─────────────────┐     ┌──────────────────┐     ┌─────────────────┐
│   API Layer     │────>│  Compliance      │────>│   Database      │
│   (Axum)        │     │  Provider        │     │   (PostgreSQL)  │
└─────────────────┘     └──────────────────┘     └────────┬────────┘
                                                          │
                        ┌──────────────────┐              │
                        │  Background      │<─────────────┘
                        │  Worker          │
                        └────────┬─────────┘
                                 │
                        ┌────────▼─────────┐
                        │  Solana RPC      │
                        │  (Helius)        │
                        └──────────────────┘
```

1. **API Layer**: Receives transfer requests via REST API
2. **Compliance Provider**: Screens recipient addresses against sanctions lists
3. **Database**: Persists all requests with their compliance and blockchain status
4. **Background Worker**: Polls for approved transactions and submits them to Solana
5. **Solana RPC**: Handles transaction submission and confirmation

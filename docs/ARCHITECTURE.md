# Architecture

Technical deep dive into the Solana Compliance Relayer's enterprise-grade architecture, data flow patterns, and reliability mechanisms.

---

## Table of Contents

- [Hexagonal Architecture](#hexagonal-architecture-ports-and-adapters)
- [High-Level Architecture Diagram](#high-level-architecture-diagram)
- [Directory Structure](#directory-structure)
- [Data Flow Diagrams](#data-flow-diagrams)
- [Transaction Lifecycle States](#transaction-lifecycle-states)
- [Enterprise Reliability Patterns](#enterprise-reliability-patterns)
- [Configuration & Provider Strategy](#configuration--provider-strategy)

---

## Hexagonal Architecture (Ports and Adapters)

The project implements **Hexagonal Architecture** to ensure clean separation between business logic and infrastructure concerns:

- **Testability:** Core business logic is independent of external systems
- **Flexibility:** Swap RPC providers, databases, or compliance APIs without changing core logic
- **Maintainability:** Clear boundaries between layers prevent coupling

### Layer Overview

| Layer | Responsibility | Location |
|-------|----------------|----------|
| **Domain** | Core business types and trait definitions (Ports) | `src/domain/` |
| **Application** | Business logic orchestration (Use Cases) | `src/app/` |
| **API** | HTTP interface (Primary Adapter) | `src/api/` |
| **Infrastructure** | External integrations (Secondary Adapters) | `src/infra/` |

---

## High-Level Architecture Diagram

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
│  │  │ Background Worker│──▶│ BlockchainClient  │──▶ Helius/QN RPC │    │
│  │  │ (SELECT FOR      │   │ (Config-Driven)   │                   │    │
│  │  │  UPDATE SKIP     │   └───────────────────┘                   │    │
│  │  │  LOCKED)         │                                           │    │
│  │  └──────────────────┘                                           │    │
│  └─────────────────────────────────────────────────────────────────┘    │
└─────────────────────────────────────────────────────────────────────────┘
```

---

## Directory Structure

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
│   └── worker.rs    # Background worker with SKIP LOCKED + fallback polling
├── api/             # HTTP interface (Primary Adapter)
│   ├── handlers.rs  # Axum route handlers with OpenAPI docs
│   ├── admin.rs     # Admin API for blocklist management
│   └── router.rs    # Rate limiting, CORS, middleware
└── infra/           # External integrations (Secondary Adapters)
    ├── database/    # PostgreSQL via SQLx (compile-time checked)
    ├── blockchain/  # Solana via Helius/QuickNode/Standard RPC
    ├── blocklist/   # Internal blocklist with DashMap + PostgreSQL
    ├── compliance/  # Range Protocol integration
    └── privacy/     # QuickNode Privacy Health Check (confidential transfers)
```

---

## Data Flow Diagrams

The transaction flow is split into two phases for clarity.

### Phase 1: Submission Flow (Receive → Persist → Process)

> **Design Principle:** Persist BEFORE compliance processing to ensure 100% auditability. If the service crashes mid-check, the record exists for recovery.

```mermaid
sequenceDiagram
    participant User as User Browser
    participant WASM as WASM Signer
    participant API as Axum API
    participant DB as PostgreSQL
    participant Range as Range Protocol

    User->>WASM: Initiate Transfer
    WASM->>WASM: Ed25519 Sign (Client-Side)
    WASM->>API: POST /transfer-requests (Signed Payload + nonce)
    
    API->>API: Verify Ed25519 Signature (message includes nonce)
    API->>API: Check Idempotency (existing nonce → return existing)
    
    rect rgb(255, 245, 200)
        Note over API,DB: STEP 1: Immediate Persistence (Audit Trail)
        API->>DB: INSERT with status: received
        DB-->>API: Record ID
    end
    
    API->>API: Check Internal Blocklist (DashMap)
    
    alt Address in Internal Blocklist
        API->>DB: UPDATE status → rejected, error: "Blocklist: reason"
        API-->>User: 200 OK {blockchain_status: "failed"}
    else Address Not Blocked
        rect rgb(200, 230, 255)
            Note over API,Range: STEP 2: Compliance Check
            API->>Range: check_compliance(address)
        end
        
        alt Address High Risk (score >= threshold)
            Range-->>API: Rejected (CRITICAL/HIGH risk)
            API->>DB: UPDATE compliance_status → rejected
            API->>API: Auto-add to Internal Blocklist
            API-->>User: 200 OK {blockchain_status: "failed"}
        else Address Clean
            Range-->>API: Approved (riskScore < threshold)
            rect rgb(200, 255, 200)
                Note over API,DB: STEP 3: Queue for Processing
                API->>DB: UPDATE compliance_status → approved, blockchain_status → pending_submission
            end
            API-->>User: 200 OK {status: "pending_submission"}
        end
    end
```

### Phase 2: Execution & Finalization Flow (with Active Polling Fallback)

> **Design Principle:** Webhooks are not 100% reliable. The system self-heals via active polling (cranks) for transactions stuck in `submitted` state.

```mermaid
sequenceDiagram
    participant DB as PostgreSQL
    participant Worker as Background Worker
    participant RPC as Helius/QuickNode RPC
    participant Webhook as Webhook Handler
    participant API as Axum API

    loop Every 10 seconds
        rect rgb(255, 230, 200)
            Note over Worker,DB: SELECT ... FOR UPDATE SKIP LOCKED
            Worker->>DB: Claim pending_submission tasks (locked)
        end
        Worker->>DB: UPDATE blockchain_status → processing
        Worker->>RPC: getLatestBlockhash()
        RPC-->>Worker: blockhash (valid ~90 seconds)
        Worker->>RPC: sendTransaction()
        
        alt Submission Success
            RPC-->>Worker: Transaction Signature
            Worker->>DB: UPDATE blockchain_status → submitted, signature, blockhash_used
            Note over Worker: Record blockhash for expiry tracking
        else Submission Failure
            Worker->>DB: Increment retry_count, exponential backoff
            Note over Worker,DB: Status remains pending_submission until max retries
        end
    end

    par Webhook Path (Primary)
        RPC->>RPC: Transaction finalized on Solana
        RPC->>Webhook: POST /webhooks/{provider}
        Webhook->>API: Parse confirmation
        API->>DB: UPDATE blockchain_status → confirmed
    and Active Polling Fallback (Crank)
        rect rgb(255, 200, 200)
            Note over Worker: Fallback for unreliable webhooks
            loop Every 60 seconds
                Worker->>DB: SELECT submitted WHERE updated_at < NOW() - INTERVAL '90 seconds'
                Worker->>RPC: getSignatureStatuses([signatures])
                alt Transaction Confirmed (finalized commitment)
                    RPC-->>Worker: status: finalized
                    Worker->>DB: UPDATE blockchain_status → confirmed
                else Transaction Not Found + Blockhash Expired
                    RPC-->>Worker: null (never landed)
                    Worker->>RPC: isBlockhashValid(blockhash_used)
                    RPC-->>Worker: false (expired)
                    Worker->>DB: UPDATE blockchain_status → expired
                    Note over Worker: Terminal state - user must re-sign
                else Transaction Failed On-Chain
                    RPC-->>Worker: err: InstructionError
                    Worker->>DB: UPDATE blockchain_status → failed, error
                end
            end
        end
    end

    Note over DB: Frontend polls GET /transfer-requests every 5s
```

---

## Transaction Lifecycle States

Transactions progress through the following states, with explicit handling for **blockhash expiry**:

```
┌──────────┐    ┌───────────────────┐     ┌────────────┐     ┌───────────┐    ┌───────────┐
│ Received │───▶│ PendingSubmission │───▶│ Processing │───▶│ Submitted │───▶│ Confirmed │
└──────────┘    └───────────────────┘     └────────────┘     └───────────┘    └───────────┘
     │                   │                      │                │                 
     │                   │                      │                │ (blockhash expired,
     ▼                   ▼                      ▼                │  tx not found)
┌──────────┐       ┌──────────┐           ┌─────────┐            │
│ Rejected │       │  Failed  │◀──────────│  Retry  │            ▼
│(blocklist│       │(10 tries │           │(backoff)│       ┌─────────┐
│ or Range)│       │ exceeded)│           └─────────┘       │ Expired │
└──────────┘       └──────────┘                             └─────────┘
                                                            (Terminal - 
                                                             re-sign required)
```

### State Transition Table

| Status | Trigger | Next State |
|--------|---------|------------|
| `received` | Initial persistence (before compliance check) | → `rejected` or `pending_submission` |
| `pending_submission` | Compliance approved, queued for worker | → `processing` |
| `processing` | Worker claimed task via SKIP LOCKED | → `submitted` (success) or retry (failure) |
| `submitted` | Transaction sent to Solana | → `confirmed` (webhook/poll) or `expired` (blockhash expired) |
| `confirmed` | Finalized commitment received | **Terminal state** |
| `expired` | Blockhash expired + tx not found | **Terminal state** (user must re-sign) |
| `failed` | Max retries (10) exceeded | **Terminal state** |

### Blockhash Expiry Handling

> **Solana Constraint:** A transaction signature is only valid for the blockhash it was built with. Blockhashes expire after ~60-90 seconds (~150 slots).

**Retry Safety Logic:**

1. **Transaction Found + Confirmed:** Update to `confirmed` (success).
2. **Transaction Found + Failed:** Update to `failed` with on-chain error.
3. **Transaction NOT Found + Blockhash Still Valid:** Wait and poll again.
4. **Transaction NOT Found + Blockhash EXPIRED:** Safe to mark as `expired`.
   - The original signature can never land; user must submit a new signed request.

---

## Enterprise Reliability Patterns

### 1. Receive → Persist → Process Pattern

**Problem:** Checking compliance before persisting loses records if the service crashes mid-check.

**Solution:** Immediately persist with status `received`, then process asynchronously.

```sql
-- Step 1: Immediate persist (API handler)
INSERT INTO transfer_requests 
    (id, from_address, to_address, ..., blockchain_status)
VALUES 
    ($1, $2, $3, ..., 'received');

-- Step 2: Update after compliance (same handler)
UPDATE transfer_requests 
SET compliance_status = 'approved', 
    blockchain_status = 'pending_submission'
WHERE id = $1;
```

### 2. SELECT ... FOR UPDATE SKIP LOCKED

**Problem:** Multiple Kubernetes pods polling the same table race to process the same transaction, causing double-submissions.

**Solution:** Atomic claim with `SKIP LOCKED`:

```sql
SELECT id, from_address, to_address, transfer_details, token_mint
FROM transfer_requests
WHERE blockchain_status = 'pending_submission'
  AND compliance_status = 'approved'
  AND (next_retry_at IS NULL OR next_retry_at <= NOW())
ORDER BY created_at ASC
LIMIT 10
FOR UPDATE SKIP LOCKED;
```

This guarantees:
- Each row is processed by exactly one worker
- Workers don't block each other (SKIP vs. wait)
- Claimed rows are invisible to other workers until committed

### 3. Active Polling Fallback (Cranks)

**Problem:** Webhooks may fail due to network issues, provider outages, or delivery delays.

**Solution:** Background crank polls `getSignatureStatuses` for transactions stuck in `submitted`:

```rust
// Pseudo-code for fallback crank
async fn poll_stale_transactions(&self) {
    let stale = db.get_submitted_older_than(Duration::from_secs(90)).await?;
    
    for tx in stale {
        let status = rpc.get_signature_status(&tx.blockchain_signature).await?;
        
        match status {
            Some(Finalized) => db.update_status(tx.id, Confirmed).await?,
            Some(Failed(err)) => db.update_status(tx.id, Failed, err).await?,
            None => {
                // Transaction not found - check blockhash
                if !rpc.is_blockhash_valid(&tx.blockhash_used).await? {
                    db.update_status(tx.id, Expired).await?;
                }
                // else: blockhash still valid, wait for next poll
            }
        }
    }
}
```

### 4. Commitment Level: Finalized

The relayer waits for **`finalized` commitment** (99.9% certainty) before marking transactions complete:

| Commitment | Certainty | Use Case |
|------------|-----------|----------|
| `processed` | ~50% | Not safe for compliance |
| `confirmed` | ~95% | Faster but can rollback |
| **`finalized`** | ~99.9% | **Required for compliance audit** |

All `getSignatureStatuses` and webhook processing verify `confirmationStatus == "finalized"`.

---

## Configuration & Provider Strategy

### Environment-Driven Provider Selection

> **Enterprise Requirement:** RPC provider type is determined explicitly via environment variables, not URL auto-detection. Enterprise endpoints often use custom domains.

| Variable | Description | Values |
|----------|-------------|--------|
| `RPC_PROVIDER_TYPE` | Explicit provider selection | `helius`, `quicknode`, `standard` |
| `SOLANA_RPC_URL` | RPC endpoint URL | Any valid Solana RPC |
| `JITO_ENABLED` | Enable Jito bundle submission | `true`, `false` |
| `JITO_TIP_LAMPORTS` | Tip amount for Jito bundles | e.g., `10000` |

### Provider Feature Matrix

| Feature | Helius | QuickNode | Standard |
|---------|--------|-----------|----------|
| Priority Fee Estimation | ✅ `getPriorityFeeEstimate` | ✅ `qn_estimatePriorityFees` | ❌ Static fallback |
| Asset Compliance (DAS) | ✅ `getAssetsByOwner` | ❌ | ❌ |
| Enhanced Webhooks | ✅ | ❌ | ❌ |
| Jito Bundles | ✅ (via Jito) | ✅ (via Jito) | ❌ |
| Privacy Health | ❌ | ✅ `qn_privacy_*` | ❌ |

### Port/Adapter Pattern

Business logic depends only on trait definitions (ports). Infrastructure implementations (adapters) are injected at startup:

```rust
// Trait (Port)
#[async_trait]
pub trait BlockchainClient: Send + Sync {
    async fn submit_transaction(&self, request: &TransferRequest) -> Result<(String, String), AppError>;
    async fn get_signature_status(&self, sig: &str) -> Result<Option<TransactionStatus>, AppError>;
    async fn is_blockhash_valid(&self, blockhash: &str) -> Result<bool, AppError>;
}

// Adapter injection at startup
let blockchain_client: Arc<dyn BlockchainClient> = match provider_type {
    ProviderType::Helius => Arc::new(HeliusClient::new(rpc_url)),
    ProviderType::QuickNode => Arc::new(QuickNodeClient::new(rpc_url)),
    ProviderType::Standard => Arc::new(StandardRpcClient::new(rpc_url)),
};
```

This enables:
- Easy testing with mock implementations
- Provider swaps without code changes
- Clear dependency boundaries

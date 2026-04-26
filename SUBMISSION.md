# S1lkPay / Frontier Submission

## Problem

Stablecoin payments are fast and cheap on Solana, but fintech operators still need compliance controls before money moves. Merchant checkout, remittance, and virtual-card funding products cannot safely integrate a raw transfer API if risky wallets can settle first and only be reviewed later.

## Solution

Solana Compliance Relayer is compliance-first payment infrastructure. It lets fintech applications accept or send Solana stablecoin payments while enforcing compliance before settlement. Clean transfers are submitted privately and reliably. Risky transfers are rejected before chain submission but retained for audit.

## Target Users

- Wallet and card apps funding virtual cards from SOL, USDC, or SPL balances
- Merchants accepting stablecoin checkout on Solana
- Cross-border remittance apps that need fast settlement and compliance evidence
- B2B treasury or payroll platforms sending many compliant stablecoin payments
- Embedded finance APIs that need auditable payment rails

## Why Solana

Solana is the execution layer because payment UX depends on low fees, fast finality, and programmable settlement. The relayer uses Solana for signed customer authorization, SPL/USDC transfers, Token-2022 confidential transfer support, low-cost retryable settlement, and private transaction submission through QuickNode/Jito when configured.

## Product Flow

1. A merchant or card program creates a checkout session with destination wallet, token mint, amount, customer wallet, expiration, and merchant reference.
2. The customer signs a transfer payload in their wallet. The signed message includes sender, recipient, amount or confidential marker, mint, and nonce.
3. The application submits the signed payload to the checkout session.
4. The relayer verifies the signature, enforces nonce replay protection, persists the transfer, checks internal blocklists, screens with Range Protocol, and queues only approved transfers.
5. A background worker submits approved transfers to Solana, with retry, blockhash expiry handling, webhook confirmation, and stale transaction polling.
6. The merchant fetches an audit report showing compliance status, settlement status, risk summary, rejection reason, signature metadata, and final machine-readable decision.

## System Architecture

- **API layer:** Axum routes for transfer requests, checkout sessions, risk checks, webhooks, admin blocklist, health checks, and OpenAPI.
- **Application layer:** `AppService` coordinates validation, compliance decisions, checkout linkage, audit reports, retry policy, and webhook processing.
- **Domain layer:** Typed transfer, checkout, compliance, blockchain, risk, and audit report models.
- **Persistence:** PostgreSQL stores transfer requests, checkout sessions, blocklist entries, risk cache, nonce/idempotency data, and Jito retry metadata.
- **Compliance:** Internal blocklist provides fast local rejection; Range Protocol screens wallet risk; Helius DAS risk data can be cached for audit.
- **Settlement:** Solana RPC client supports SOL, SPL token, Token-2022 confidential transfers, QuickNode/Jito private submission, retries, and confirmation handling.

## Security and Compliance Model

- Ed25519 signature verification proves the sender authorized the transfer payload.
- Nonces are persisted and used as idempotency keys to prevent replay.
- Transfers are persisted before compliance screening so rejected transfers remain auditable.
- Internal blocklist rejects known risky sender or recipient wallets before external calls.
- Range Protocol can reject high-risk addresses before settlement.
- Rejected transfers are never submitted to chain.
- `/admin/*` routes require `ADMIN_API_KEY` in production.
- Helius and QuickNode webhooks fail closed with `401 Unauthorized` when configured secrets are missing or mismatched.
- QuickNode/Jito submission avoids public mempool exposure when private submission is enabled.
- Stale transaction polling protects against missed webhooks.

## Demo Script

1. Start PostgreSQL and run `cargo run`.
2. Open Swagger at `http://localhost:3000/swagger-ui`.
3. Create a USDC checkout session with `POST /checkout/sessions`.
4. Submit a signed matching transfer with `POST /checkout/sessions/{id}/submit-transfer`.
5. Fetch `GET /checkout/sessions/{id}` and show the linked transfer status.
6. Fetch `GET /transfer-requests/{id}/audit-report` and explain the final decision.
7. Add a suspicious wallet through authenticated `/admin/blocklist`.
8. Submit a transfer to that wallet and show that it is rejected before settlement but retained for audit.

## Commercial Potential

This can become a payment infrastructure API for wallets, card programs, merchants, and remittance companies. The near-term product is an API layer for stablecoin checkout and virtual-card funding with compliance evidence. Longer term, it can support merchant dashboards, hosted checkout pages, webhooks for payment lifecycle events, KYB/KYC integrations, transaction monitoring, and volume-based pricing for fintech operators.

## Post-Hackathon Roadmap

- Merchant API keys and per-merchant isolation
- Hosted checkout page and wallet adapter integration
- Payment lifecycle webhooks for merchants
- USDC-first presets and token allowlists
- KYB/KYC provider integration for merchant onboarding
- Compliance case management and exportable audit bundles
- Production observability, SLOs, alerts, and incident runbooks
- Multi-region deployment and provider failover
- Settlement reconciliation reports for card issuers and treasury teams

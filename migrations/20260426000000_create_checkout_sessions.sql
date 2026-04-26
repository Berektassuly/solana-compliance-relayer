-- Merchant checkout and virtual-card funding sessions.
-- Sessions let payment applications create a durable invoice/funding intent,
-- then link the customer's signed Solana transfer request for compliance,
-- settlement, and audit.

CREATE TABLE IF NOT EXISTS checkout_sessions (
    id TEXT PRIMARY KEY,
    merchant_id TEXT NOT NULL,
    merchant_reference TEXT NOT NULL,
    destination_wallet TEXT NOT NULL,
    token_mint TEXT,
    amount BIGINT NOT NULL CHECK (amount > 0),
    customer_wallet TEXT,
    status TEXT NOT NULL DEFAULT 'open' CHECK (
        status IN (
            'open',
            'transfer_submitted',
            'settled',
            'rejected',
            'expired',
            'failed'
        )
    ),
    expires_at TIMESTAMPTZ NOT NULL,
    merchant_metadata JSONB,
    transfer_request_id TEXT REFERENCES transfer_requests(id) ON DELETE SET NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_checkout_sessions_merchant_reference
    ON checkout_sessions(merchant_id, merchant_reference);

CREATE INDEX IF NOT EXISTS idx_checkout_sessions_transfer_request
    ON checkout_sessions(transfer_request_id);

CREATE INDEX IF NOT EXISTS idx_checkout_sessions_status_expires
    ON checkout_sessions(status, expires_at);

COMMENT ON TABLE checkout_sessions IS
    'Merchant checkout and virtual-card funding sessions linked to compliance-screened Solana transfer requests';

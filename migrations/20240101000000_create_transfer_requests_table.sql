-- Create transfer_requests table
CREATE TABLE IF NOT EXISTS transfer_requests (
    id VARCHAR(255) PRIMARY KEY,
    from_address VARCHAR(255) NOT NULL,
    to_address VARCHAR(255) NOT NULL,
    amount_sol DOUBLE PRECISION NOT NULL,
    compliance_status VARCHAR(50) NOT NULL DEFAULT 'pending',
    blockchain_status VARCHAR(50) NOT NULL DEFAULT 'pending',
    blockchain_signature VARCHAR(255),
    blockchain_retry_count INTEGER NOT NULL DEFAULT 0,
    blockchain_last_error TEXT,
    blockchain_next_retry_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- Indexes for frequently accessed fields
CREATE INDEX IF NOT EXISTS idx_transfer_requests_created_at ON transfer_requests(created_at DESC);
CREATE INDEX IF NOT EXISTS idx_transfer_requests_from ON transfer_requests(from_address);
CREATE INDEX IF NOT EXISTS idx_transfer_requests_to ON transfer_requests(to_address);
CREATE INDEX IF NOT EXISTS idx_transfer_requests_status ON transfer_requests(blockchain_status);
CREATE INDEX IF NOT EXISTS idx_transfer_requests_retry ON transfer_requests(blockchain_next_retry_at) 
    WHERE blockchain_status IN ('pending', 'pending_submission');
CREATE INDEX IF NOT EXISTS idx_transfer_requests_compliance ON transfer_requests(compliance_status);

-- Comment on table
COMMENT ON TABLE transfer_requests IS 'Core table for storing SOL transfer requests and their compliance/blockchain status';

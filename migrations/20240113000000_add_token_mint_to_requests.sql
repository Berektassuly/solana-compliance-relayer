-- Add token_mint column for SPL Token transfers
-- NULL means native SOL transfer, otherwise contains the mint address

ALTER TABLE transfer_requests ADD COLUMN token_mint VARCHAR(255);

-- Index for efficient queries by token type
CREATE INDEX IF NOT EXISTS idx_transfer_requests_token_mint ON transfer_requests(token_mint);

-- Documentation
COMMENT ON COLUMN transfer_requests.token_mint IS 'SPL Token mint address for token transfers, NULL for native SOL transfers';

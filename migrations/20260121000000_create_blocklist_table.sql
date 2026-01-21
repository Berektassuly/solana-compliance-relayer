-- Create blocklist table for persistent address screening
-- Addresses in this table are blocked from all transfers (as sender or recipient)

CREATE TABLE IF NOT EXISTS blocklist (
    address TEXT PRIMARY KEY,
    reason TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- Index for efficient lookups
CREATE INDEX IF NOT EXISTS idx_blocklist_created_at ON blocklist(created_at);

-- Seed with initial known malicious address
INSERT INTO blocklist (address, reason) 
VALUES (
    '4oS78GPe66RqBduuAeiMFANf27FpmgXNwokZ3ocN4z1B',
    'Internal Security Alert: Address linked to Phishing Scam (Flagged manually)'
) ON CONFLICT (address) DO NOTHING;

-- Comment for documentation
COMMENT ON TABLE blocklist IS 'Internal blocklist for screening malicious wallet addresses before external compliance checks';

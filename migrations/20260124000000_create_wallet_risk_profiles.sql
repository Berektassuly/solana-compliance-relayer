-- Create wallet_risk_profiles table for caching aggregated risk analytics
-- Reduces API costs by caching Range Protocol and Helius DAS results

CREATE TABLE IF NOT EXISTS wallet_risk_profiles (
    address TEXT PRIMARY KEY,
    -- Range Protocol data
    risk_score INTEGER,
    risk_level TEXT,
    reasoning TEXT,
    -- Helius DAS data
    has_sanctioned_assets BOOLEAN NOT NULL DEFAULT FALSE,
    helius_assets_checked BOOLEAN NOT NULL DEFAULT FALSE,
    -- Timestamps
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- Index for efficient cache expiry queries
CREATE INDEX IF NOT EXISTS idx_wallet_risk_profiles_updated_at 
    ON wallet_risk_profiles(updated_at);

-- Comment for documentation
COMMENT ON TABLE wallet_risk_profiles IS 'Cached wallet risk profiles from Range Protocol and Helius DAS for pre-flight compliance checks';

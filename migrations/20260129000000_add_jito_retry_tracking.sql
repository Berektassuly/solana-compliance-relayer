-- Migration: Add Jito Retry Tracking
-- Phase 2 of Security Remediation: Jito Double Spend Protection
--
-- This migration adds columns to track the original transaction signature,
-- last error type, and blockhash used. This enables safe retry logic when
-- a Jito bundle returns JitoStateUnknown (ambiguous state where the 
-- transaction may or may not have been processed).
--
-- The workflow:
-- 1. On submission, store original_tx_signature and blockhash_used
-- 2. On JitoStateUnknown error, store last_error_type = 'jito_state_unknown'
-- 3. On retry, if last_error_type = 'jito_state_unknown':
--    - Query blockchain for original_tx_signature status
--    - If confirmed/finalized: mark as success (no retry needed)
--    - If blockhash expired and tx not found: safe to retry with new blockhash
--    - If blockhash still valid: wait longer before retrying

-- Add original transaction signature for tracking across retries
ALTER TABLE transfer_requests ADD COLUMN original_tx_signature VARCHAR(128);

-- Add last error type classification for smart retry logic
-- Values: 'none', 'jito_state_unknown', 'jito_bundle_failed', 
--         'transaction_failed', 'network_error', 'validation_error'
ALTER TABLE transfer_requests ADD COLUMN last_error_type VARCHAR(50);

-- Add blockhash used in the last transaction attempt
-- Used to check if blockhash has expired (>150 slots = safe to retry)
ALTER TABLE transfer_requests ADD COLUMN blockhash_used VARCHAR(64);

-- Index for finding requests by error type (useful for monitoring/debugging)
CREATE INDEX idx_transfer_requests_error_type ON transfer_requests(last_error_type);

-- Add comment for documentation
COMMENT ON COLUMN transfer_requests.original_tx_signature IS 'Original transaction signature stored on first submission, used to check status before retry';
COMMENT ON COLUMN transfer_requests.last_error_type IS 'Classification of last error: none, jito_state_unknown, jito_bundle_failed, transaction_failed, network_error, validation_error';
COMMENT ON COLUMN transfer_requests.blockhash_used IS 'Blockhash used in last transaction attempt, used to determine if safe to retry with new blockhash';

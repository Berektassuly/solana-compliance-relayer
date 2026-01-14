-- Financial Precision Refactor: Convert amount_sol from DOUBLE PRECISION to BIGINT
-- 
-- IMPORTANT: This migration truncates the table because existing data contains
-- mixed SOL (9 decimals) and token (variable decimals) amounts that cannot be
-- uniformly converted to atomic units.

-- Clear existing data (safe for development)
TRUNCATE TABLE transfer_requests;

-- Change column type from DOUBLE PRECISION to BIGINT
ALTER TABLE transfer_requests 
    ALTER COLUMN amount_sol TYPE BIGINT USING 0;

-- Rename column from amount_sol to amount
ALTER TABLE transfer_requests 
    RENAME COLUMN amount_sol TO amount;

-- Update column comment
COMMENT ON COLUMN transfer_requests.amount IS 'Transfer amount in atomic units (lamports for SOL, raw units for SPL tokens)';

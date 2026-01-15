-- Add columns for Confidential Transfers
ALTER TABLE transfer_requests 
ADD COLUMN transfer_type TEXT,
ADD COLUMN proof_data TEXT,
ADD COLUMN encrypted_amount TEXT;

-- Backfill existing rows as 'public'
UPDATE transfer_requests 
SET transfer_type = 'public' 
WHERE transfer_type IS NULL;

-- Enforce transfer_type is not null (after backfill)
ALTER TABLE transfer_requests 
ALTER COLUMN transfer_type SET NOT NULL;

-- Make amount nullable (Confidential transfers don't have a public amount)
-- But only do this after we've secured the type for existing rows
ALTER TABLE transfer_requests 
ALTER COLUMN amount DROP NOT NULL;

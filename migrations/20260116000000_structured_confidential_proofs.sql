-- Migrate to structured confidential transfer proofs (Smart Relayer pattern)
-- Rename and add columns for individual proof components

-- Rename proof_data to equality_proof (first proof component)
ALTER TABLE transfer_requests 
RENAME COLUMN proof_data TO equality_proof;

-- Rename encrypted_amount to new_decryptable_available_balance
ALTER TABLE transfer_requests 
RENAME COLUMN encrypted_amount TO new_decryptable_available_balance;

-- Add the two new proof columns
ALTER TABLE transfer_requests 
ADD COLUMN ciphertext_validity_proof TEXT,
ADD COLUMN range_proof TEXT;

-- Migration: add_request_nonce.sql
-- Phase 3: Replay Attack & Idempotency Protection (Unified Request Uniqueness System)
-- 
-- This migration adds nonce and client_signature columns to prevent:
-- 1. Replay attacks: Same signed payload cannot be reused (nonce in signature message)
-- 2. Duplicate requests: Network retries won't create duplicate transfers (unique constraint)

-- Add nonce column for request uniqueness
ALTER TABLE transfer_requests ADD COLUMN nonce VARCHAR(64);

-- Add client_signature column to store the original signed request
-- This allows verification and auditing of the original client signature
ALTER TABLE transfer_requests ADD COLUMN client_signature VARCHAR(128);

-- Unique constraint on nonce prevents duplicate nonces
-- Uses partial index (WHERE nonce IS NOT NULL) to allow existing rows with NULL nonce
-- This enables backward compatibility during migration
CREATE UNIQUE INDEX idx_transfer_requests_nonce 
    ON transfer_requests(nonce) 
    WHERE nonce IS NOT NULL;

-- Composite index for fast lookups by from_address and nonce
-- Used by find_by_nonce() to check for existing requests
CREATE INDEX idx_transfer_requests_from_nonce 
    ON transfer_requests(from_address, nonce);

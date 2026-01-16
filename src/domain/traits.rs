//! Domain traits defining contracts for external systems.

use async_trait::async_trait;

use super::error::AppError;
use super::types::{
    BlockchainStatus, ComplianceStatus, PaginatedResponse, SubmitTransferRequest, TransferRequest,
};
use chrono::{DateTime, Utc};

/// Compliance provider trait for screening requests
#[async_trait]
pub trait ComplianceProvider: Send + Sync {
    /// Check if a transfer request is compliant
    async fn check_compliance(
        &self,
        request: &SubmitTransferRequest,
    ) -> Result<ComplianceStatus, AppError>;
}

/// Database client trait for persistence operations
#[async_trait]
pub trait DatabaseClient: Send + Sync {
    /// Check database connectivity
    async fn health_check(&self) -> Result<(), AppError>;

    /// Get a single transfer request by ID
    async fn get_transfer_request(&self, id: &str) -> Result<Option<TransferRequest>, AppError>;

    /// Submit a new transfer request
    async fn submit_transfer(
        &self,
        data: &SubmitTransferRequest,
    ) -> Result<TransferRequest, AppError>;

    /// List transfer requests with cursor-based pagination
    async fn list_transfer_requests(
        &self,
        limit: i64,
        cursor: Option<&str>,
    ) -> Result<PaginatedResponse<TransferRequest>, AppError>;

    /// Update blockchain status for a transfer request
    async fn update_blockchain_status(
        &self,
        id: &str,
        status: BlockchainStatus,
        signature: Option<&str>,
        error: Option<&str>,
        next_retry_at: Option<DateTime<Utc>>,
    ) -> Result<(), AppError>;

    /// Update compliance status for a transfer request
    async fn update_compliance_status(
        &self,
        id: &str,
        status: crate::domain::ComplianceStatus,
    ) -> Result<(), AppError>;

    /// Get requests pending blockchain submission
    async fn get_pending_blockchain_requests(
        &self,
        limit: i64,
    ) -> Result<Vec<TransferRequest>, AppError>;

    /// Increment retry count for a request
    async fn increment_retry_count(&self, id: &str) -> Result<i32, AppError>;

    /// Get a transfer request by blockchain signature
    async fn get_transfer_by_signature(
        &self,
        signature: &str,
    ) -> Result<Option<TransferRequest>, AppError>;
}

/// Blockchain client trait for chain operations
#[async_trait]
pub trait BlockchainClient: Send + Sync {
    /// Check blockchain RPC connectivity
    async fn health_check(&self) -> Result<(), AppError>;

    /// Submit a transaction using the transfer request details
    async fn submit_transaction(&self, request: &TransferRequest) -> Result<String, AppError>;

    /// Get transaction confirmation status
    async fn get_transaction_status(&self, signature: &str) -> Result<bool, AppError> {
        let _ = signature;
        Err(AppError::NotSupported(
            "get_transaction_status not implemented".to_string(),
        ))
    }

    /// Get current block height
    async fn get_block_height(&self) -> Result<u64, AppError> {
        Err(AppError::NotSupported(
            "get_block_height not implemented".to_string(),
        ))
    }

    /// Get latest blockhash for transaction construction
    async fn get_latest_blockhash(&self) -> Result<String, AppError> {
        Err(AppError::NotSupported(
            "get_latest_blockhash not implemented".to_string(),
        ))
    }

    /// Wait for transaction confirmation with timeout
    async fn wait_for_confirmation(
        &self,
        signature: &str,
        timeout_secs: u64,
    ) -> Result<bool, AppError> {
        let _ = (signature, timeout_secs);
        Err(AppError::NotSupported(
            "wait_for_confirmation not implemented".to_string(),
        ))
    }

    /// Transfer SOL from the issuer wallet to a destination address
    /// Amount is in lamports (1 SOL = 1_000_000_000 lamports)
    /// Returns the transaction signature on success
    async fn transfer_sol(
        &self,
        to_address: &str,
        amount_lamports: u64,
    ) -> Result<String, AppError> {
        let _ = (to_address, amount_lamports);
        Err(AppError::NotSupported(
            "transfer_sol not implemented".to_string(),
        ))
    }

    /// Transfer SPL Tokens from the issuer wallet to a destination address
    /// Creates the destination ATA if it doesn't exist
    /// Amount is in raw token units (caller must pre-convert using token decimals)
    /// Example: 1 USDC (6 decimals) = 1_000_000 raw units
    /// Returns the transaction signature on success
    async fn transfer_token(
        &self,
        to_address: &str,
        token_mint: &str,
        amount: u64,
    ) -> Result<String, AppError> {
        let _ = (to_address, token_mint, amount);
        Err(AppError::NotSupported(
            "transfer_token not implemented".to_string(),
        ))
    }

    /// Transfer Token-2022 Confidential tokens
    /// The server constructs the instruction from structured proof components,
    /// ensuring full control over what it signs (mitigates Confused Deputy).
    async fn transfer_confidential(
        &self,
        to_address: &str,
        token_mint: &str,
        new_decryptable_available_balance: &str,
        equality_proof: &str,
        ciphertext_validity_proof: &str,
        range_proof: &str,
    ) -> Result<String, AppError> {
        let _ = (
            to_address,
            token_mint,
            new_decryptable_available_balance,
            equality_proof,
            ciphertext_validity_proof,
            range_proof,
        );
        Err(AppError::NotSupported(
            "transfer_confidential not implemented".to_string(),
        ))
    }

    /// Check if a wallet holds compliant assets using DAS (Digital Asset Standard).
    /// This is a Helius-specific feature for compliance screening.
    ///
    /// Returns `false` if the wallet holds assets from sanctioned collections.
    /// For non-Helius providers, returns `true` (skip check / assume compliant).
    ///
    /// # Arguments
    /// * `owner` - The wallet address (Base58) to check
    async fn check_wallet_assets(&self, owner: &str) -> Result<bool, AppError> {
        let _ = owner;
        // Default: skip check for providers without DAS support
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Minimal implementation for testing default methods
    #[allow(dead_code)]
    struct MinimalDatabaseClient;

    #[async_trait]
    impl DatabaseClient for MinimalDatabaseClient {
        async fn health_check(&self) -> Result<(), AppError> {
            Ok(())
        }

        async fn get_transfer_request(
            &self,
            _id: &str,
        ) -> Result<Option<TransferRequest>, AppError> {
            Ok(None)
        }

        async fn submit_transfer(
            &self,
            _data: &SubmitTransferRequest,
        ) -> Result<TransferRequest, AppError> {
            Ok(TransferRequest::default())
        }

        async fn list_transfer_requests(
            &self,
            _limit: i64,
            _cursor: Option<&str>,
        ) -> Result<PaginatedResponse<TransferRequest>, AppError> {
            Ok(PaginatedResponse::empty())
        }

        async fn update_blockchain_status(
            &self,
            _id: &str,
            _status: BlockchainStatus,
            _signature: Option<&str>,
            _error: Option<&str>,
            _next_retry_at: Option<DateTime<Utc>>,
        ) -> Result<(), AppError> {
            Ok(())
        }

        async fn update_compliance_status(
            &self,
            _id: &str,
            _status: crate::domain::ComplianceStatus,
        ) -> Result<(), AppError> {
            Ok(())
        }

        async fn get_pending_blockchain_requests(
            &self,
            _limit: i64,
        ) -> Result<Vec<TransferRequest>, AppError> {
            Ok(vec![])
        }

        async fn increment_retry_count(&self, _id: &str) -> Result<i32, AppError> {
            Ok(1)
        }

        async fn get_transfer_by_signature(
            &self,
            _signature: &str,
        ) -> Result<Option<TransferRequest>, AppError> {
            Ok(None)
        }
    }

    struct MinimalBlockchainClient;

    #[async_trait]
    impl BlockchainClient for MinimalBlockchainClient {
        async fn health_check(&self) -> Result<(), AppError> {
            Ok(())
        }

        async fn submit_transaction(&self, _request: &TransferRequest) -> Result<String, AppError> {
            Ok("sig_123".to_string())
        }
    }

    #[tokio::test]
    async fn test_blockchain_client_get_transaction_status_not_supported() {
        let client = MinimalBlockchainClient;
        let result = client.get_transaction_status("sig").await;
        assert!(matches!(result, Err(AppError::NotSupported(_))));
    }
}

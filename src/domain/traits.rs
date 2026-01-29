//! Domain traits defining contracts for external systems.

use async_trait::async_trait;

use super::error::AppError;
use super::types::{
    BlockchainStatus, ComplianceStatus, LastErrorType, PaginatedResponse, SubmitTransferRequest,
    TransactionStatus, TransferRequest, WalletRiskProfile,
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

    // =========================================================================
    // Jito Double Spend Protection Methods
    // =========================================================================

    /// Update Jito tracking fields for a transfer request.
    /// Used to store the original signature, error type, and blockhash for safe retry logic.
    async fn update_jito_tracking(
        &self,
        id: &str,
        original_tx_signature: Option<&str>,
        last_error_type: LastErrorType,
        blockhash_used: Option<&str>,
    ) -> Result<(), AppError> {
        let _ = (id, original_tx_signature, last_error_type, blockhash_used);
        Ok(())
    }

    // =========================================================================
    // Risk Profile Methods (for pre-flight compliance screening cache)
    // =========================================================================

    /// Get a cached risk profile for a wallet address.
    /// Returns None if not cached or cache expired (updated_at older than max_age_secs).
    async fn get_risk_profile(
        &self,
        address: &str,
        max_age_secs: i64,
    ) -> Result<Option<WalletRiskProfile>, AppError> {
        let _ = (address, max_age_secs);
        Ok(None)
    }

    /// Upsert a risk profile for a wallet address.
    async fn upsert_risk_profile(&self, profile: &WalletRiskProfile) -> Result<(), AppError> {
        let _ = profile;
        Ok(())
    }
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

    // =========================================================================
    // Jito Double Spend Protection Methods
    // =========================================================================

    /// Query the status of a transaction by its signature.
    /// Used to verify if an original transaction was processed before retrying
    /// after a JitoStateUnknown error.
    ///
    /// # Returns
    /// - `Ok(Some(TransactionStatus::Confirmed))` - Transaction confirmed
    /// - `Ok(Some(TransactionStatus::Finalized))` - Transaction finalized
    /// - `Ok(Some(TransactionStatus::Failed(msg)))` - Transaction failed on-chain
    /// - `Ok(None)` - Transaction not found (may still be processing or never submitted)
    async fn get_signature_status(
        &self,
        signature: &str,
    ) -> Result<Option<TransactionStatus>, AppError> {
        let _ = signature;
        Err(AppError::NotSupported(
            "get_signature_status not implemented".to_string(),
        ))
    }

    /// Check if a blockhash is still valid (not expired).
    /// Blockhashes typically expire after ~150 slots (~1-2 minutes).
    ///
    /// If the blockhash is expired and the original transaction was not found,
    /// it's safe to retry with a new blockhash (the original tx cannot be
    /// processed anymore).
    ///
    /// # Returns
    /// - `Ok(true)` - Blockhash is still valid
    /// - `Ok(false)` - Blockhash has expired
    async fn is_blockhash_valid(&self, blockhash: &str) -> Result<bool, AppError> {
        let _ = blockhash;
        Err(AppError::NotSupported(
            "is_blockhash_valid not implemented".to_string(),
        ))
    }

    /// Classify a blockchain error into LastErrorType for retry logic.
    /// This helper method categorizes errors to determine safe retry strategies.
    fn classify_error(&self, error: &AppError) -> LastErrorType {
        match error {
            AppError::Blockchain(crate::domain::BlockchainError::JitoStateUnknown(_)) => {
                LastErrorType::JitoStateUnknown
            }
            AppError::Blockchain(crate::domain::BlockchainError::JitoBundleFailed(_)) => {
                LastErrorType::JitoBundleFailed
            }
            AppError::Blockchain(crate::domain::BlockchainError::TransactionFailed(_)) => {
                LastErrorType::TransactionFailed
            }
            AppError::Blockchain(
                crate::domain::BlockchainError::Connection(_)
                | crate::domain::BlockchainError::Timeout(_)
                | crate::domain::BlockchainError::RpcError(_),
            ) => LastErrorType::NetworkError,
            AppError::Validation(_) => LastErrorType::ValidationError,
            _ => LastErrorType::TransactionFailed,
        }
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

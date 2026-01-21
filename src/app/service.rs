//! Application service layer with graceful degradation.

use chrono::{Duration, Utc};
use std::sync::Arc;
use tracing::{error, info, instrument, warn};
use validator::Validate;

use crate::domain::{
    AppError, BlockchainClient, BlockchainStatus, ComplianceStatus, DatabaseClient, HealthResponse,
    HealthStatus, HeliusTransaction, PaginatedResponse, SubmitTransferRequest, TransferRequest,
    ValidationError,
};
use crate::infra::BlocklistManager;

/// Maximum number of retry attempts for blockchain submission
const MAX_RETRY_ATTEMPTS: i32 = 10;

/// Maximum backoff duration in seconds (5 minutes)
const MAX_BACKOFF_SECS: i64 = 300;

/// Application service containing business logic
pub struct AppService {
    db_client: Arc<dyn DatabaseClient>,
    blockchain_client: Arc<dyn BlockchainClient>,
    compliance_provider: Arc<dyn crate::domain::ComplianceProvider>,
    /// Optional internal blocklist for fast local screening
    blocklist: Option<Arc<BlocklistManager>>,
}

impl AppService {
    #[must_use]
    pub fn new(
        db_client: Arc<dyn DatabaseClient>,
        blockchain_client: Arc<dyn BlockchainClient>,
        compliance_provider: Arc<dyn crate::domain::ComplianceProvider>,
    ) -> Self {
        Self {
            db_client,
            blockchain_client,
            compliance_provider,
            blocklist: None,
        }
    }

    /// Create AppService with blocklist manager
    #[must_use]
    pub fn with_blocklist(
        db_client: Arc<dyn DatabaseClient>,
        blockchain_client: Arc<dyn BlockchainClient>,
        compliance_provider: Arc<dyn crate::domain::ComplianceProvider>,
        blocklist: Arc<BlocklistManager>,
    ) -> Self {
        Self {
            db_client,
            blockchain_client,
            compliance_provider,
            blocklist: Some(blocklist),
        }
    }

    /// Submit a new transfer request for background processing.
    /// Validates, checks compliance, persists to database, and returns immediately.
    /// Blockchain submission is handled asynchronously by background workers.
    #[instrument(skip(self, request), fields(from = %request.from_address, to = %request.to_address))]
    pub async fn submit_transfer(
        &self,
        request: &SubmitTransferRequest,
    ) -> Result<TransferRequest, AppError> {
        // Cryptographic signature verification (MUST be first - before any state changes)
        request.verify_signature().map_err(|e| {
            warn!(from = %request.from_address, error = %e, "Signature verification failed");
            e
        })?;

        request.validate().map_err(|e| {
            warn!(error = %e, "Validation failed");
            AppError::Validation(ValidationError::Multiple(e.to_string()))
        })?;

        info!("Submitting new transfer request");

        // Internal blocklist check (fast O(1) lookup - before external API call)
        // Check both sender and recipient addresses
        if let Some(ref blocklist) = self.blocklist {
            // Check recipient first (more common case)
            if let Some(reason) = blocklist.check_address(&request.to_address) {
                warn!(
                    address = %request.to_address,
                    reason = %reason,
                    "Transfer blocked: recipient in internal blocklist"
                );
                // Persist with rejected status and store the reason
                let mut transfer_request = self.db_client.submit_transfer(request).await?;
                self.db_client
                    .update_compliance_status(&transfer_request.id, ComplianceStatus::Rejected)
                    .await?;
                // Store the blocklist reason and mark as failed
                self.db_client
                    .update_blockchain_status(
                        &transfer_request.id,
                        BlockchainStatus::Failed,
                        None,
                        Some(&format!("Blocklist: {}", reason)),
                        None,
                    )
                    .await?;
                transfer_request.compliance_status = ComplianceStatus::Rejected;
                transfer_request.blockchain_status = BlockchainStatus::Failed;
                transfer_request.blockchain_last_error = Some(format!("Blocklist: {}", reason));
                return Ok(transfer_request);
            }

            // Check sender address
            if let Some(reason) = blocklist.check_address(&request.from_address) {
                warn!(
                    address = %request.from_address,
                    reason = %reason,
                    "Transfer blocked: sender in internal blocklist"
                );
                // Persist with rejected status and store the reason
                let mut transfer_request = self.db_client.submit_transfer(request).await?;
                self.db_client
                    .update_compliance_status(&transfer_request.id, ComplianceStatus::Rejected)
                    .await?;
                // Store the blocklist reason and mark as failed
                self.db_client
                    .update_blockchain_status(
                        &transfer_request.id,
                        BlockchainStatus::Failed,
                        None,
                        Some(&format!("Blocklist: {}", reason)),
                        None,
                    )
                    .await?;
                transfer_request.compliance_status = ComplianceStatus::Rejected;
                transfer_request.blockchain_status = BlockchainStatus::Failed;
                transfer_request.blockchain_last_error = Some(format!("Blocklist: {}", reason));
                return Ok(transfer_request);
            }
        }

        // External compliance check (synchronous - slower)
        let compliance_status = self.compliance_provider.check_compliance(request).await?;
        if compliance_status == crate::domain::ComplianceStatus::Rejected {
            warn!(from = %request.from_address, to = %request.to_address, "Transfer rejected by compliance provider");
        }

        // Persist to database (single source of truth)
        let mut transfer_request = self.db_client.submit_transfer(request).await?;

        // Update compliance status
        if compliance_status != crate::domain::ComplianceStatus::Pending {
            self.db_client
                .update_compliance_status(&transfer_request.id, compliance_status)
                .await?;
        }
        transfer_request.compliance_status = compliance_status;

        // If rejected, return early - no blockchain submission needed
        if compliance_status == crate::domain::ComplianceStatus::Rejected {
            return Ok(transfer_request);
        }

        // Queue for background processing (Outbox Pattern: no blockchain call here!)
        self.db_client
            .update_blockchain_status(
                &transfer_request.id,
                BlockchainStatus::PendingSubmission,
                None,
                None,
                None,
            )
            .await?;
        transfer_request.blockchain_status = BlockchainStatus::PendingSubmission;

        info!(id = %transfer_request.id, "Transfer accepted for background processing");

        Ok(transfer_request)
    }

    /// Get a transfer request by ID
    #[instrument(skip(self))]
    pub async fn get_transfer_request(
        &self,
        id: &str,
    ) -> Result<Option<TransferRequest>, AppError> {
        self.db_client.get_transfer_request(id).await
    }

    /// List transfer requests with pagination
    #[instrument(skip(self))]
    pub async fn list_transfer_requests(
        &self,
        limit: i64,
        cursor: Option<&str>,
    ) -> Result<PaginatedResponse<TransferRequest>, AppError> {
        self.db_client.list_transfer_requests(limit, cursor).await
    }

    /// Retry blockchain submission for a specific request
    #[instrument(skip(self))]
    pub async fn retry_blockchain_submission(&self, id: &str) -> Result<TransferRequest, AppError> {
        let transfer_request = self
            .db_client
            .get_transfer_request(id)
            .await?
            .ok_or_else(|| {
                AppError::Database(crate::domain::DatabaseError::NotFound(id.to_string()))
            })?;

        // SECURITY: Block retry if compliance was rejected (unless it was a blocklist rejection and address is now clear)
        if transfer_request.compliance_status == ComplianceStatus::Rejected {
            // Check if this was a blocklist rejection
            let was_blocklist_rejection = transfer_request
                .blockchain_last_error
                .as_ref()
                .map(|e| e.starts_with("Blocklist:"))
                .unwrap_or(false);

            if was_blocklist_rejection {
                // Re-check blocklist - if address is now clear, allow retry
                let mut is_still_blocked = false;

                if let Some(ref blocklist) = self.blocklist {
                    if blocklist
                        .check_address(&transfer_request.to_address)
                        .is_some()
                    {
                        is_still_blocked = true;
                    }
                    if blocklist
                        .check_address(&transfer_request.from_address)
                        .is_some()
                    {
                        is_still_blocked = true;
                    }
                }

                if is_still_blocked {
                    warn!(
                        id = %id,
                        "Retry blocked: address still in blocklist"
                    );
                    return Err(AppError::Validation(ValidationError::InvalidField {
                        field: "compliance_status".to_string(),
                        message: "Address is still blocklisted".to_string(),
                    }));
                }

                // Address is now clear - update compliance status to approved
                info!(
                    id = %id,
                    "Blocklist cleared: updating compliance status to approved for retry"
                );
                self.db_client
                    .update_compliance_status(id, ComplianceStatus::Approved)
                    .await?;
            } else {
                // Non-blocklist rejection - cannot retry
                warn!(
                    id = %id,
                    "Retry blocked: compliance status is rejected (not blocklist)"
                );
                return Err(AppError::Validation(ValidationError::InvalidField {
                    field: "compliance_status".to_string(),
                    message: "Cannot retry a rejected transfer".to_string(),
                }));
            }
        }

        // SECURITY: Re-check blocklist before allowing retry (for non-rejected transfers)
        if let Some(ref blocklist) = self.blocklist {
            if let Some(reason) = blocklist.check_address(&transfer_request.to_address) {
                warn!(
                    id = %id,
                    address = %transfer_request.to_address,
                    reason = %reason,
                    "Retry blocked: recipient in blocklist"
                );
                return Err(AppError::Validation(ValidationError::InvalidField {
                    field: "to_address".to_string(),
                    message: format!("Recipient address is blocklisted: {}", reason),
                }));
            }
            if let Some(reason) = blocklist.check_address(&transfer_request.from_address) {
                warn!(
                    id = %id,
                    address = %transfer_request.from_address,
                    reason = %reason,
                    "Retry blocked: sender in blocklist"
                );
                return Err(AppError::Validation(ValidationError::InvalidField {
                    field: "from_address".to_string(),
                    message: format!("Sender address is blocklisted: {}", reason),
                }));
            }
        }

        if transfer_request.blockchain_status != BlockchainStatus::PendingSubmission
            && transfer_request.blockchain_status != BlockchainStatus::Failed
        {
            return Err(AppError::Validation(ValidationError::InvalidField {
                field: "blockchain_status".to_string(),
                message: "Request is not pending submission or failed".to_string(),
            }));
        }

        match self
            .blockchain_client
            .submit_transaction(&transfer_request)
            .await
        {
            Ok(signature) => {
                info!(id = %transfer_request.id, signature = %signature, "Retry submission successful");
                self.db_client
                    .update_blockchain_status(
                        id,
                        BlockchainStatus::Submitted,
                        Some(&signature),
                        None,
                        None,
                    )
                    .await?;
                let mut updated_request = transfer_request;
                updated_request.blockchain_status = BlockchainStatus::Submitted;
                updated_request.blockchain_signature = Some(signature);
                updated_request.blockchain_last_error = None;
                updated_request.blockchain_next_retry_at = None;
                Ok(updated_request)
            }
            Err(e) => {
                warn!(id = %transfer_request.id, error = ?e, "Retry submission failed");
                let retry_count = self.db_client.increment_retry_count(id).await?;
                let (status, next_retry) = if retry_count >= MAX_RETRY_ATTEMPTS {
                    (BlockchainStatus::Failed, None)
                } else {
                    let backoff = calculate_backoff(retry_count);
                    (
                        BlockchainStatus::PendingSubmission,
                        Some(Utc::now() + Duration::seconds(backoff)),
                    )
                };

                self.db_client
                    .update_blockchain_status(id, status, None, Some(&e.to_string()), next_retry)
                    .await?;

                Err(e)
            }
        }
    }

    /// Process pending blockchain submissions (called by background worker)
    #[instrument(skip(self))]
    pub async fn process_pending_submissions(&self, batch_size: i64) -> Result<usize, AppError> {
        let pending_requests = self
            .db_client
            .get_pending_blockchain_requests(batch_size)
            .await?;
        let count = pending_requests.len();

        if count == 0 {
            return Ok(0);
        }

        info!(count = count, "Processing pending blockchain submissions");

        for request in pending_requests {
            if let Err(e) = self.process_single_submission(&request).await {
                error!(id = %request.id, error = ?e, "Failed to process pending submission");
            }
        }

        Ok(count)
    }

    /// Process a single pending submission
    async fn process_single_submission(&self, request: &TransferRequest) -> Result<(), AppError> {
        // Defense in depth: Skip non-approved requests (should be filtered at DB level already)
        if request.compliance_status != ComplianceStatus::Approved {
            warn!(id = %request.id, status = ?request.compliance_status, "Skipping non-approved request");
            return Ok(());
        }

        // Delegate dispatch to blockchain client
        let result = self.blockchain_client.submit_transaction(request).await;

        match result {
            Ok(signature) => {
                let transfer_type = if request.token_mint.is_some() {
                    "Token"
                } else {
                    "SOL"
                };
                info!(id = %request.id, signature = %signature, r#type = %transfer_type, "Transfer successful");
                self.db_client
                    .update_blockchain_status(
                        &request.id,
                        BlockchainStatus::Submitted,
                        Some(&signature),
                        None,
                        None,
                    )
                    .await?;
            }
            Err(e) => {
                let transfer_type = if request.token_mint.is_some() {
                    "Token"
                } else {
                    "SOL"
                };
                warn!(id = %request.id, error = ?e, r#type = %transfer_type, "Transfer failed");
                let retry_count = self.db_client.increment_retry_count(&request.id).await?;
                let (status, next_retry) = if retry_count >= MAX_RETRY_ATTEMPTS {
                    (BlockchainStatus::Failed, None)
                } else {
                    let backoff = calculate_backoff(retry_count);
                    (
                        BlockchainStatus::PendingSubmission,
                        Some(Utc::now() + Duration::seconds(backoff)),
                    )
                };

                self.db_client
                    .update_blockchain_status(
                        &request.id,
                        status,
                        None,
                        Some(&e.to_string()),
                        next_retry,
                    )
                    .await?;
            }
        }

        Ok(())
    }

    /// Perform health check on all dependencies
    #[instrument(skip(self))]
    pub async fn health_check(&self) -> HealthResponse {
        let db_health = match self.db_client.health_check().await {
            Ok(()) => HealthStatus::Healthy,
            Err(_) => HealthStatus::Unhealthy,
        };
        let blockchain_health = match self.blockchain_client.health_check().await {
            Ok(()) => HealthStatus::Healthy,
            Err(_) => HealthStatus::Unhealthy,
        };
        HealthResponse::new(db_health, blockchain_health)
    }

    /// Process incoming Helius webhook transactions.
    /// Updates blockchain status for transactions we have initiated.
    /// Returns the number of transactions actually processed.
    #[instrument(skip(self, transactions), fields(tx_count = %transactions.len()))]
    pub async fn process_helius_webhook(
        &self,
        transactions: Vec<HeliusTransaction>,
    ) -> Result<usize, AppError> {
        let mut processed = 0;

        for tx in transactions {
            // Look up by signature to see if this is one of our transactions
            if let Some(request) = self
                .db_client
                .get_transfer_by_signature(&tx.signature)
                .await?
            {
                // Only update if currently in Submitted status (waiting for confirmation)
                if request.blockchain_status == BlockchainStatus::Submitted {
                    let (new_status, error_msg) = if tx.transaction_error.is_none() {
                        info!(id = %request.id, signature = %tx.signature, "Transaction confirmed via Helius webhook");
                        (BlockchainStatus::Confirmed, None)
                    } else {
                        let err = tx
                            .transaction_error
                            .as_ref()
                            .map(|e| e.to_string())
                            .unwrap_or_else(|| "Unknown transaction error".to_string());
                        warn!(id = %request.id, signature = %tx.signature, error = %err, "Transaction failed via Helius webhook");
                        (BlockchainStatus::Failed, Some(err))
                    };

                    self.db_client
                        .update_blockchain_status(
                            &request.id,
                            new_status,
                            None,
                            error_msg.as_deref(),
                            None,
                        )
                        .await?;

                    processed += 1;
                }
            }
        }

        info!(processed = %processed, "Helius webhook processing complete");
        Ok(processed)
    }
}

/// Calculate exponential backoff with maximum cap
fn calculate_backoff(retry_count: i32) -> i64 {
    let backoff = 2_i64.pow(retry_count.min(8) as u32);
    backoff.min(MAX_BACKOFF_SECS)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_calculate_backoff() {
        assert_eq!(calculate_backoff(0), 1);
        assert_eq!(calculate_backoff(1), 2);
        assert_eq!(calculate_backoff(2), 4);
        assert_eq!(calculate_backoff(3), 8);
        assert_eq!(calculate_backoff(4), 16);
        assert_eq!(calculate_backoff(5), 32);
        assert_eq!(calculate_backoff(6), 64);
        assert_eq!(calculate_backoff(7), 128);
        assert_eq!(calculate_backoff(8), 256);
        assert_eq!(calculate_backoff(9), 256); // Capped at 2^8
        assert_eq!(calculate_backoff(10), 256);
    }
}

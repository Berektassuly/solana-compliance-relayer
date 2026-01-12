//! Application service layer with graceful degradation.

use chrono::{Duration, Utc};
use std::sync::Arc;
use tracing::{error, info, instrument, warn};
use validator::Validate;

use crate::domain::{
    AppError, BlockchainClient, BlockchainStatus, DatabaseClient,
    HealthResponse, HealthStatus, PaginatedResponse, SubmitTransferRequest, TransferRequest,
    ValidationError,
};

/// Maximum number of retry attempts for blockchain submission
const MAX_RETRY_ATTEMPTS: i32 = 10;

/// Maximum backoff duration in seconds (5 minutes)
const MAX_BACKOFF_SECS: i64 = 300;

/// Application service containing business logic
pub struct AppService {
    db_client: Arc<dyn DatabaseClient>,
    blockchain_client: Arc<dyn BlockchainClient>,
}

impl AppService {
    #[must_use]
    pub fn new(
        db_client: Arc<dyn DatabaseClient>,
        blockchain_client: Arc<dyn BlockchainClient>,
    ) -> Self {
        Self {
            db_client,
            blockchain_client,
        }
    }

    /// Submit a new transfer request and attempt blockchain submission.
    /// If blockchain is unavailable, stores request with pending_submission status.
    #[instrument(skip(self, request), fields(from = %request.from_address, to = %request.to_address))]
    pub async fn submit_transfer(
        &self,
        request: &SubmitTransferRequest,
    ) -> Result<TransferRequest, AppError> {
        request.validate().map_err(|e| {
            warn!(error = %e, "Validation failed");
            AppError::Validation(ValidationError::Multiple(e.to_string()))
        })?;

        info!("Submitting new transfer request");
        let mut transfer_request = self.db_client.submit_transfer(request).await?;
        info!(id = %transfer_request.id, "Transfer request created in database");

        // Attempt blockchain submission with graceful degradation
        match self
            .blockchain_client
            .submit_transaction(&transfer_request)
            .await
        {
            Ok(signature) => {
                info!(id = %transfer_request.id, signature = %signature, "Submitted to blockchain");
                self.db_client
                    .update_blockchain_status(
                        &transfer_request.id,
                        BlockchainStatus::Submitted,
                        Some(&signature),
                        None,
                        None,
                    )
                    .await?;
                transfer_request.blockchain_status = BlockchainStatus::Submitted;
                transfer_request.blockchain_signature = Some(signature);
            }
            Err(e) => {
                warn!(id = %transfer_request.id, error = ?e, "Blockchain submission failed, queuing for retry");
                let next_retry = Utc::now() + Duration::seconds(1);
                self.db_client
                    .update_blockchain_status(
                        &transfer_request.id,
                        BlockchainStatus::PendingSubmission,
                        None,
                        Some(&e.to_string()),
                        Some(next_retry),
                    )
                    .await?;
                transfer_request.blockchain_status = BlockchainStatus::PendingSubmission;
                transfer_request.blockchain_last_error = Some(e.to_string());
                transfer_request.blockchain_next_retry_at = Some(next_retry);
            }
        }

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
    pub async fn retry_blockchain_submission(
        &self,
        id: &str,
    ) -> Result<TransferRequest, AppError> {
        let transfer_request = self
            .db_client
            .get_transfer_request(id)
            .await?
            .ok_or_else(|| {
                AppError::Database(crate::domain::DatabaseError::NotFound(id.to_string()))
            })?;

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
        match self.blockchain_client.submit_transaction(request).await {
            Ok(signature) => {
                info!(id = %request.id, signature = %signature, "Background submission successful");
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
                warn!(id = %request.id, error = ?e, "Background submission failed");
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

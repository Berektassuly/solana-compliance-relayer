//! Application service layer with graceful degradation.

use chrono::{Duration, Utc};
use std::sync::Arc;
use tracing::{error, info, instrument, warn};
use validator::Validate;

use crate::domain::{
    AppError, BlockchainClient, BlockchainStatus, ComplianceStatus, DatabaseClient, HealthResponse,
    HealthStatus, HeliusTransaction, LastErrorType, PaginatedResponse, QuickNodeWebhookEvent,
    SubmitTransferRequest, TransactionStatus, TransferRequest, ValidationError,
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

    // =========================================================================
    // Request Uniqueness Methods (Replay Protection & Idempotency)
    // =========================================================================

    /// Find an existing request by from_address and nonce.
    /// Used to check for duplicate requests (idempotency) and prevent replay attacks.
    ///
    /// # Arguments
    /// * `from_address` - The sender's wallet address
    /// * `nonce` - The unique nonce from the request
    ///
    /// # Returns
    /// - `Ok(Some(TransferRequest))` - Existing request found with this nonce
    /// - `Ok(None)` - No existing request with this nonce
    #[instrument(skip(self))]
    pub async fn find_by_nonce(
        &self,
        from_address: &str,
        nonce: &str,
    ) -> Result<Option<TransferRequest>, AppError> {
        self.db_client.find_by_nonce(from_address, nonce).await
    }

    /// Submit a new transfer request for background processing.
    /// Validates, checks compliance, persists to database, and returns immediately.
    /// Blockchain submission is handled asynchronously by background workers.
    ///
    /// ## Replay Protection & Idempotency
    /// The `nonce` field in the request must be unique per sender address.
    /// - If a request with the same (from_address, nonce) already exists, the existing
    ///   request is returned (idempotent behavior).
    /// - The nonce is included in the signature message to prevent replay attacks:
    ///   `{from}:{to}:{amount|confidential}:{mint|SOL}:{nonce}`
    #[instrument(skip(self, request), fields(from = %request.from_address, to = %request.to_address, nonce = %request.nonce))]
    pub async fn submit_transfer(
        &self,
        request: &SubmitTransferRequest,
    ) -> Result<TransferRequest, AppError> {
        // Validation first (includes nonce format validation)
        request.validate().map_err(|e| {
            warn!(error = %e, "Validation failed");
            AppError::Validation(ValidationError::Multiple(e.to_string()))
        })?;

        // Cryptographic signature verification (now includes nonce in message)
        // Format: "{from}:{to}:{amount|confidential}:{mint|SOL}:{nonce}"
        request.verify_signature().map_err(|e| {
            warn!(from = %request.from_address, nonce = %request.nonce, error = %e, "Signature verification failed");
            e
        })?;

        // Defense in depth: Check for existing request with same nonce
        // (Primary check is in API handler, this is secondary protection)
        if let Some(existing) = self
            .find_by_nonce(&request.from_address, &request.nonce)
            .await?
        {
            info!(
                nonce = %request.nonce,
                existing_id = %existing.id,
                "Idempotent return: existing request found for nonce (defense in depth)"
            );
            return Ok(existing);
        }

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

        // If rejected by Range, persist with Failed status AND auto-add to blocklist
        if compliance_status == crate::domain::ComplianceStatus::Rejected {
            warn!(from = %request.from_address, to = %request.to_address, "Transfer rejected by compliance provider");

            let rejection_reason = "Range Protocol: High-risk address detected (CRITICAL RISK)";

            // Persist transfer with rejected status
            let mut transfer_request = self.db_client.submit_transfer(request).await?;
            self.db_client
                .update_compliance_status(&transfer_request.id, ComplianceStatus::Rejected)
                .await?;

            // Mark as Failed with reason (same pattern as blocklist)
            self.db_client
                .update_blockchain_status(
                    &transfer_request.id,
                    BlockchainStatus::Failed,
                    None,
                    Some(rejection_reason),
                    None,
                    None,
                )
                .await?;

            // Auto-add to internal blocklist to avoid future API calls
            if let Some(ref blocklist) = self.blocklist
                && blocklist.check_address(&request.to_address).is_none()
            {
                info!(
                    address = %request.to_address,
                    "Auto-adding high-risk address to internal blocklist"
                );
                let _ = blocklist
                    .add_address(
                        request.to_address.clone(),
                        "Auto-blocked: Range Protocol CRITICAL RISK".to_string(),
                    )
                    .await;
            }

            transfer_request.compliance_status = ComplianceStatus::Rejected;
            transfer_request.blockchain_status = BlockchainStatus::Failed;
            transfer_request.blockchain_last_error = Some(rejection_reason.to_string());
            return Ok(transfer_request);
        }

        // Compliance passed - persist to database (single source of truth)
        let mut transfer_request = self.db_client.submit_transfer(request).await?;
        self.db_client
            .update_compliance_status(&transfer_request.id, ComplianceStatus::Approved)
            .await?;
        transfer_request.compliance_status = ComplianceStatus::Approved;

        // Queue for background processing (Outbox Pattern: no blockchain call here!)
        self.db_client
            .update_blockchain_status(
                &transfer_request.id,
                BlockchainStatus::PendingSubmission,
                None,
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

        // =====================================================================
        // JITO DOUBLE SPEND PROTECTION
        // =====================================================================
        // If the previous error was JitoStateUnknown, check if the original
        // transaction was processed before attempting a retry.
        // =====================================================================

        if transfer_request.last_error_type == LastErrorType::JitoStateUnknown
            && let Some(original_sig) = transfer_request.original_tx_signature.clone()
        {
            info!(
                id = %transfer_request.id,
                original_sig = %original_sig,
                "Checking original transaction status before manual retry (JitoStateUnknown)"
            );

            match self
                .blockchain_client
                .get_signature_status(&original_sig)
                .await
            {
                Ok(Some(TransactionStatus::Confirmed | TransactionStatus::Finalized)) => {
                    // Original transaction landed! Update as success, no retry needed
                    info!(
                        id = %transfer_request.id,
                        original_sig = %original_sig,
                        "Original transaction confirmed - marking as success (prevented double-spend)"
                    );
                    self.db_client
                        .update_blockchain_status(
                            id,
                            BlockchainStatus::Submitted,
                            Some(&original_sig),
                            None,
                            None,
                            transfer_request.blockhash_used.as_deref(),
                        )
                        .await?;
                    self.db_client
                        .update_jito_tracking(id, None, LastErrorType::None, None)
                        .await?;

                    let mut updated_request = transfer_request;
                    updated_request.blockchain_status = BlockchainStatus::Submitted;
                    updated_request.blockchain_signature = Some(original_sig);
                    updated_request.blockchain_last_error = None;
                    updated_request.last_error_type = LastErrorType::None;
                    return Ok(updated_request);
                }
                Ok(Some(TransactionStatus::Failed(_))) | Ok(None) => {
                    // Failed or not found - safe to retry with new blockhash
                    info!(
                        id = %transfer_request.id,
                        "Original tx not confirmed - proceeding with retry"
                    );
                    self.db_client
                        .update_jito_tracking(id, None, LastErrorType::None, None)
                        .await?;
                }
                Err(e) => {
                    warn!(
                        id = %transfer_request.id,
                        error = %e,
                        "Failed to check original tx status, proceeding with retry"
                    );
                }
            }
        }

        match self
            .blockchain_client
            .submit_transaction(&transfer_request)
            .await
        {
            Ok((signature, blockhash)) => {
                info!(id = %transfer_request.id, signature = %signature, "Retry submission successful");
                self.db_client
                    .update_blockchain_status(
                        id,
                        BlockchainStatus::Submitted,
                        Some(&signature),
                        None,
                        None,
                        Some(&blockhash),
                    )
                    .await?;
                self.db_client
                    .update_jito_tracking(id, None, LastErrorType::None, Some(&blockhash))
                    .await?;
                let mut updated_request = transfer_request;
                updated_request.blockchain_status = BlockchainStatus::Submitted;
                updated_request.blockchain_signature = Some(signature.clone());
                updated_request.blockhash_used = Some(blockhash);
                updated_request.blockchain_last_error = None;
                updated_request.blockchain_next_retry_at = None;
                updated_request.last_error_type = LastErrorType::None;
                Ok(updated_request)
            }
            Err(e) => {
                let error_type = self.blockchain_client.classify_error(&e);
                warn!(id = %transfer_request.id, error = ?e, error_type = %error_type, "Retry submission failed");
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
                    .update_blockchain_status(
                        id,
                        status,
                        None,
                        Some(&e.to_string()),
                        next_retry,
                        None,
                    )
                    .await?;

                // Store Jito tracking info
                let original_sig = transfer_request.blockchain_signature.as_deref();
                self.db_client
                    .update_jito_tracking(id, original_sig, error_type, None)
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

    /// Process a single pending submission with Jito Double Spend Protection.
    ///
    /// This method implements the Jito Double Spend Protection:
    /// - Before retrying after a JitoStateUnknown error, check if the original
    ///   transaction was processed to prevent double-spend.
    /// - Track the error type to enable smart retry logic.
    async fn process_single_submission(&self, request: &TransferRequest) -> Result<(), AppError> {
        // Defense in depth: Skip non-approved requests (should be filtered at DB level already)
        if request.compliance_status != ComplianceStatus::Approved {
            warn!(id = %request.id, status = ?request.compliance_status, "Skipping non-approved request");
            return Ok(());
        }

        // =====================================================================
        // JITO DOUBLE SPEND PROTECTION
        // =====================================================================
        // If the previous error was JitoStateUnknown, we MUST check if the
        // original transaction was processed before attempting a retry.
        // Otherwise, we risk double-spending if the original tx actually landed.
        // =====================================================================

        if request.last_error_type == LastErrorType::JitoStateUnknown
            && let Some(ref original_sig) = request.original_tx_signature
        {
            info!(
                id = %request.id,
                original_sig = %original_sig,
                "Checking original transaction status before retry (JitoStateUnknown)"
            );

            // Query blockchain for transaction status
            match self
                .blockchain_client
                .get_signature_status(original_sig)
                .await
            {
                Ok(Some(TransactionStatus::Confirmed | TransactionStatus::Finalized)) => {
                    // Original transaction landed! Update as success, no retry needed
                    info!(
                        id = %request.id,
                        original_sig = %original_sig,
                        "Original transaction confirmed - marking as success (prevented double-spend)"
                    );
                    self.db_client
                        .update_blockchain_status(
                            &request.id,
                            BlockchainStatus::Submitted,
                            Some(original_sig),
                            None,
                            None,
                            request.blockhash_used.as_deref(),
                        )
                        .await?;
                    // Clear error type since tx succeeded
                    self.db_client
                        .update_jito_tracking(&request.id, None, LastErrorType::None, None)
                        .await?;
                    return Ok(());
                }
                Ok(Some(TransactionStatus::Failed(err))) => {
                    // Definite failure, safe to retry with new blockhash
                    info!(
                        id = %request.id,
                        original_sig = %original_sig,
                        error = %err,
                        "Original tx failed on-chain - safe to retry with new blockhash"
                    );
                    // Update error type to indicate safe retry
                    self.db_client
                        .update_jito_tracking(
                            &request.id,
                            None,
                            LastErrorType::TransactionFailed,
                            None,
                        )
                        .await?;
                }
                Ok(None) => {
                    // Transaction not found - check if blockhash has expired
                    if let Some(ref blockhash) = request.blockhash_used {
                        let blockhash_valid = self
                            .blockchain_client
                            .is_blockhash_valid(blockhash)
                            .await
                            .unwrap_or(false);

                        if blockhash_valid {
                            // Blockhash still valid, tx might still land - wait longer
                            info!(
                                id = %request.id,
                                blockhash = %blockhash,
                                "Blockhash still valid, waiting longer before retry"
                            );

                            // Schedule a retry with backoff
                            let retry_count =
                                self.db_client.increment_retry_count(&request.id).await?;
                            let backoff = calculate_backoff(retry_count);
                            self.db_client
                                .update_blockchain_status(
                                    &request.id,
                                    BlockchainStatus::PendingSubmission,
                                    None,
                                    Some("JitoStateUnknown: waiting for blockhash expiry"),
                                    Some(Utc::now() + Duration::seconds(backoff)),
                                    None,
                                )
                                .await?;
                            return Ok(());
                        }
                    }
                    // Blockhash expired and tx not found = safe to retry with new blockhash
                    info!(
                        id = %request.id,
                        "Blockhash expired and tx not found - safe to retry with new blockhash"
                    );
                    self.db_client
                        .update_jito_tracking(&request.id, None, LastErrorType::NetworkError, None)
                        .await?;
                }
                Err(e) => {
                    // Error checking status - log and continue with caution
                    warn!(
                        id = %request.id,
                        error = %e,
                        "Failed to check original tx status, proceeding with caution"
                    );
                }
            }
        }

        // Delegate dispatch to blockchain client
        let result = self.blockchain_client.submit_transaction(request).await;

        match result {
            Ok((signature, blockhash)) => {
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
                        Some(&blockhash),
                    )
                    .await?;
                // Clear Jito tracking on success (persist blockhash for future retry logic)
                self.db_client
                    .update_jito_tracking(&request.id, None, LastErrorType::None, Some(&blockhash))
                    .await?;
            }
            Err(e) => {
                let transfer_type = if request.token_mint.is_some() {
                    "Token"
                } else {
                    "SOL"
                };

                // Classify the error for smart retry logic
                let error_type = self.blockchain_client.classify_error(&e);
                warn!(
                    id = %request.id,
                    error = ?e,
                    error_type = %error_type,
                    r#type = %transfer_type,
                    "Transfer failed"
                );

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
                        None,
                    )
                    .await?;

                // Store Jito tracking info for JitoStateUnknown errors
                // This enables status check on next retry attempt
                if error_type == LastErrorType::JitoStateUnknown {
                    // Extract signature and blockhash from error context if available
                    // For now, we store the error type - signature is obtained from
                    // blockchain_signature if previously set
                    let original_sig = request.blockchain_signature.as_deref();
                    self.db_client
                        .update_jito_tracking(&request.id, original_sig, error_type, None)
                        .await?;
                } else {
                    // Update error type for non-Jito errors
                    self.db_client
                        .update_jito_tracking(&request.id, None, error_type, None)
                        .await?;
                }
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

    /// Process incoming QuickNode webhook events.
    /// Updates blockchain status for transactions we have initiated.
    ///
    /// **IMPORTANT**: QuickNode webhooks can deliver an array of events in a single POST.
    /// This method processes ALL events in the batch, not just a single event.
    ///
    /// Returns the number of transactions actually processed (status updated).
    #[instrument(skip(self, events), fields(event_count = %events.len()))]
    pub async fn process_quicknode_webhook(
        &self,
        events: Vec<QuickNodeWebhookEvent>,
    ) -> Result<usize, AppError> {
        let mut processed = 0;

        // Process ALL events in the batch (not 1:1 mapping of request to event)
        for event in events {
            // Look up by signature to see if this is one of our transactions
            if let Some(request) = self
                .db_client
                .get_transfer_by_signature(&event.signature)
                .await?
            {
                // Only update if currently in Submitted status (waiting for confirmation)
                if request.blockchain_status == BlockchainStatus::Submitted {
                    let (new_status, error_msg) = if event.is_success() {
                        info!(
                            id = %request.id,
                            signature = %event.signature,
                            slot = ?event.slot,
                            "Transaction confirmed via QuickNode webhook"
                        );
                        (BlockchainStatus::Confirmed, None)
                    } else {
                        let err = event
                            .error_message()
                            .unwrap_or_else(|| "Unknown transaction error".to_string());
                        warn!(
                            id = %request.id,
                            signature = %event.signature,
                            error = %err,
                            "Transaction failed via QuickNode webhook"
                        );
                        (BlockchainStatus::Failed, Some(err))
                    };

                    self.db_client
                        .update_blockchain_status(
                            &request.id,
                            new_status,
                            None,
                            error_msg.as_deref(),
                            None,
                            None,
                        )
                        .await?;

                    processed += 1;
                }
            }
        }

        info!(processed = %processed, "QuickNode webhook processing complete");
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

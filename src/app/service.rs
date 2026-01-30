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
    /// Implements the **Receive → Persist → Process** pattern for 100% auditability.
    ///
    /// ## Flow
    /// 1. Validate request and verify signature
    /// 2. Check idempotency (return existing if nonce matches)
    /// 3. **PERSIST immediately** with status `Received` (audit trail)
    /// 4. Run compliance checks (blocklist + Range Protocol)
    /// 5. Update status to `PendingSubmission` (approved) or `Failed` (rejected)
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
        // =====================================================================
        // STEP 1: Validation (before any persistence)
        // =====================================================================
        request.validate().map_err(|e| {
            warn!(error = %e, "Validation failed");
            AppError::Validation(ValidationError::Multiple(e.to_string()))
        })?;

        // Cryptographic signature verification (includes nonce in message)
        // Format: "{from}:{to}:{amount|confidential}:{mint|SOL}:{nonce}"
        request.verify_signature().map_err(|e| {
            warn!(from = %request.from_address, nonce = %request.nonce, error = %e, "Signature verification failed");
            e
        })?;

        // Check for existing request with same nonce (idempotency)
        if let Some(existing) = self
            .find_by_nonce(&request.from_address, &request.nonce)
            .await?
        {
            info!(
                nonce = %request.nonce,
                existing_id = %existing.id,
                "Idempotent return: existing request found for nonce"
            );
            return Ok(existing);
        }

        // =====================================================================
        // STEP 2: PERSIST IMMEDIATELY (Audit Trail - before compliance check!)
        // =====================================================================
        // This ensures 100% auditability: if the service crashes during compliance
        // check, the record still exists in the database with status `Received`.
        info!("Persisting transfer request with status 'received'");
        let mut transfer_request = self.db_client.submit_transfer(request).await?;
        let request_id = transfer_request.id.clone();

        // =====================================================================
        // STEP 3: Compliance Checks (blocklist + Range Protocol)
        // =====================================================================

        // Internal blocklist check (fast O(1) lookup)
        if let Some(ref blocklist) = self.blocklist {
            // Check recipient
            if let Some(reason) = blocklist.check_address(&request.to_address) {
                warn!(
                    address = %request.to_address,
                    reason = %reason,
                    "Transfer blocked: recipient in internal blocklist"
                );
                return self
                    .reject_transfer(&request_id, &format!("Blocklist: {}", reason))
                    .await;
            }

            // Check sender
            if let Some(reason) = blocklist.check_address(&request.from_address) {
                warn!(
                    address = %request.from_address,
                    reason = %reason,
                    "Transfer blocked: sender in internal blocklist"
                );
                return self
                    .reject_transfer(&request_id, &format!("Blocklist: {}", reason))
                    .await;
            }
        }

        // External compliance check (Range Protocol - slower, external API)
        let compliance_status = self.compliance_provider.check_compliance(request).await?;

        if compliance_status == crate::domain::ComplianceStatus::Rejected {
            warn!(from = %request.from_address, to = %request.to_address, "Transfer rejected by compliance provider");

            let rejection_reason = "Range Protocol: High-risk address detected (CRITICAL RISK)";

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

            return self.reject_transfer(&request_id, rejection_reason).await;
        }

        // =====================================================================
        // STEP 4: Approve and Queue for Background Processing
        // =====================================================================
        self.db_client
            .update_compliance_status(&request_id, ComplianceStatus::Approved)
            .await?;
        transfer_request.compliance_status = ComplianceStatus::Approved;

        // Queue for background worker (Outbox Pattern: no blockchain call here!)
        self.db_client
            .update_blockchain_status(
                &request_id,
                BlockchainStatus::PendingSubmission,
                None,
                None,
                None,
                None,
            )
            .await?;
        transfer_request.blockchain_status = BlockchainStatus::PendingSubmission;

        info!(id = %transfer_request.id, "Transfer approved and queued for background processing");

        Ok(transfer_request)
    }

    /// Internal helper to reject a transfer request (used after persist)
    async fn reject_transfer(&self, id: &str, reason: &str) -> Result<TransferRequest, AppError> {
        self.db_client
            .update_compliance_status(id, ComplianceStatus::Rejected)
            .await?;
        self.db_client
            .update_blockchain_status(id, BlockchainStatus::Failed, None, Some(reason), None, None)
            .await?;

        // Fetch and return the updated request
        self.db_client
            .get_transfer_request(id)
            .await?
            .ok_or_else(|| {
                AppError::Database(crate::domain::DatabaseError::NotFound(id.to_string()))
            })
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
                    // SAFETY: Cannot verify original tx status due to RPC/network error.
                    // We MUST NOT submit a new transaction - the original might have landed.
                    // Reschedule for later retry of the status check instead.
                    error!(
                        id = %request.id,
                        error = %e,
                        "Failed to check original tx status - rescheduling (cannot safely retry)"
                    );

                    let retry_count = self.db_client.increment_retry_count(&request.id).await?;
                    let backoff = calculate_backoff(retry_count);
                    self.db_client
                        .update_blockchain_status(
                            &request.id,
                            BlockchainStatus::PendingSubmission,
                            None,
                            Some(&format!(
                                "JitoStateUnknown: RPC error checking status - {}",
                                e
                            )),
                            Some(Utc::now() + Duration::seconds(backoff)),
                            None,
                        )
                        .await?;

                    // Return Ok to indicate we handled this request (rescheduled, not failed)
                    // The request stays in PendingSubmission and will be retried later
                    return Ok(());
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

    // =========================================================================
    // Active Polling Fallback (Crank) for Stale Submitted Transactions
    // =========================================================================

    /// Process transactions stuck in `submitted` state (Active Polling Fallback / Crank).
    ///
    /// This method implements the fallback for webhook unreliability:
    /// 1. Query DB for transactions in `submitted` status older than `older_than_secs`
    /// 2. For each transaction, check on-chain status via `getSignatureStatuses`
    /// 3. Update status based on result:
    ///    - Confirmed/Finalized → `Confirmed`
    ///    - Not Found + Blockhash Expired → `Expired` (terminal, user must re-sign)
    ///    - Not Found + Blockhash Valid → Wait (next crank cycle)
    ///
    /// This is a self-healing mechanism that handles webhook failures.
    #[instrument(skip(self))]
    pub async fn process_stale_submitted_transactions(
        &self,
        older_than_secs: i64,
        batch_size: i64,
    ) -> Result<usize, AppError> {
        let stale_transactions = self
            .db_client
            .get_stale_submitted_transactions(older_than_secs, batch_size)
            .await?;

        let count = stale_transactions.len();
        if count == 0 {
            return Ok(0);
        }

        info!(
            count = count,
            older_than_secs = older_than_secs,
            "Processing stale submitted transactions (crank)"
        );

        for tx in stale_transactions {
            if let Err(e) = self.check_stale_transaction_status(&tx).await {
                error!(id = %tx.id, error = ?e, "Failed to check stale transaction status");
            }
        }

        Ok(count)
    }

    /// Check the on-chain status of a single stale submitted transaction.
    async fn check_stale_transaction_status(&self, tx: &TransferRequest) -> Result<(), AppError> {
        let signature = match &tx.blockchain_signature {
            Some(sig) => sig,
            None => {
                // No signature stored - shouldn't happen for submitted status
                warn!(id = %tx.id, "Stale transaction has no signature - marking as failed");
                self.db_client
                    .update_blockchain_status(
                        &tx.id,
                        BlockchainStatus::Failed,
                        None,
                        Some("No blockchain signature found for submitted transaction"),
                        None,
                        None,
                    )
                    .await?;
                return Ok(());
            }
        };

        info!(id = %tx.id, signature = %signature, "Checking stale transaction status on-chain");

        // Query blockchain for transaction status
        match self.blockchain_client.get_signature_status(signature).await {
            Ok(Some(TransactionStatus::Confirmed | TransactionStatus::Finalized)) => {
                // Transaction confirmed! Webhook missed it.
                info!(
                    id = %tx.id,
                    signature = %signature,
                    "Stale transaction confirmed on-chain (webhook missed)"
                );
                self.db_client
                    .update_blockchain_status(
                        &tx.id,
                        BlockchainStatus::Confirmed,
                        Some(signature),
                        None,
                        None,
                        tx.blockhash_used.as_deref(),
                    )
                    .await?;
            }
            Ok(Some(TransactionStatus::Failed(err))) => {
                // Transaction failed on-chain
                warn!(id = %tx.id, error = %err, "Stale transaction failed on-chain");
                self.db_client
                    .update_blockchain_status(
                        &tx.id,
                        BlockchainStatus::Failed,
                        Some(signature),
                        Some(&format!("Transaction failed on-chain: {}", err)),
                        None,
                        tx.blockhash_used.as_deref(),
                    )
                    .await?;
            }
            Ok(None) => {
                // Transaction not found - check if blockhash expired
                info!(id = %tx.id, "Transaction not found on-chain - checking blockhash validity");
                self.handle_not_found_transaction(tx, signature).await?;
            }
            Err(e) => {
                // RPC error - log and leave for next crank cycle
                warn!(id = %tx.id, error = ?e, "Failed to query transaction status - will retry next cycle");
            }
        }

        Ok(())
    }

    /// Handle a transaction that was not found on-chain.
    /// If blockhash is expired, mark as `Expired` (terminal state).
    async fn handle_not_found_transaction(
        &self,
        tx: &TransferRequest,
        signature: &str,
    ) -> Result<(), AppError> {
        let blockhash = match &tx.blockhash_used {
            Some(bh) => bh,
            None => {
                // No blockhash stored - can't determine expiry, leave for next cycle
                warn!(id = %tx.id, "No blockhash stored - cannot determine expiry");
                return Ok(());
            }
        };

        // Check if blockhash is still valid
        match self.blockchain_client.is_blockhash_valid(blockhash).await {
            Ok(true) => {
                // Blockhash still valid - transaction might still land
                info!(id = %tx.id, "Blockhash still valid - transaction may still land");
                // Leave in submitted state, will check again next cycle
            }
            Ok(false) => {
                // Blockhash expired + transaction not found = transaction will never land
                // This is a TERMINAL state - user must re-sign with fresh nonce
                warn!(
                    id = %tx.id,
                    signature = %signature,
                    blockhash = %blockhash,
                    "Blockhash expired and transaction not found - marking as EXPIRED"
                );
                self.db_client
                    .update_blockchain_status(
                        &tx.id,
                        BlockchainStatus::Expired,
                        Some(signature),
                        Some("Transaction blockhash expired - please re-sign and submit with fresh nonce"),
                        None,
                        Some(blockhash),
                    )
                    .await?;
            }
            Err(e) => {
                // RPC error - log and leave for next cycle
                warn!(id = %tx.id, error = ?e, "Failed to check blockhash validity - will retry next cycle");
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

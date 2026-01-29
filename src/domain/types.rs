//! Domain types with validation support.

use chrono::{DateTime, Utc};
use ed25519_dalek::{Signature, VerifyingKey};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use validator::Validate;

use crate::domain::AppError;

/// Status of blockchain submission for a transfer
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum BlockchainStatus {
    /// Initial state, not yet processed
    #[default]
    Pending,
    /// Waiting to be submitted to blockchain (queued for worker)
    PendingSubmission,
    /// Worker has claimed this task, processing in progress
    Processing,
    /// Transaction submitted, awaiting confirmation
    Submitted,
    /// Transaction confirmed on blockchain
    Confirmed,
    /// Submission failed after max retries
    Failed,
}

impl BlockchainStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::PendingSubmission => "pending_submission",
            Self::Processing => "processing",
            Self::Submitted => "submitted",
            Self::Confirmed => "confirmed",
            Self::Failed => "failed",
        }
    }
}

impl std::str::FromStr for BlockchainStatus {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "pending" => Ok(Self::Pending),
            "pending_submission" => Ok(Self::PendingSubmission),
            "processing" => Ok(Self::Processing),
            "submitted" => Ok(Self::Submitted),
            "confirmed" => Ok(Self::Confirmed),
            "failed" => Ok(Self::Failed),
            _ => Err(format!("Invalid blockchain status: {}", s)),
        }
    }
}

impl std::fmt::Display for BlockchainStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

/// Compliance status for a transfer
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ComplianceStatus {
    /// Initial state, waiting for compliance check
    #[default]
    Pending,
    /// Compliance check passed
    Approved,
    /// Compliance check failed
    Rejected,
}

impl ComplianceStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Approved => "approved",
            Self::Rejected => "rejected",
        }
    }
}

impl std::str::FromStr for ComplianceStatus {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "pending" => Ok(Self::Pending),
            "approved" => Ok(Self::Approved),
            "rejected" => Ok(Self::Rejected),
            _ => Err(format!("Invalid compliance status: {}", s)),
        }
    }
}

impl std::fmt::Display for ComplianceStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

// ============================================================================
// Jito Double Spend Protection Types
// ============================================================================

/// Classification of the last error encountered during blockchain submission.
/// Used for smart retry logic to prevent double-spend on JitoStateUnknown.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum LastErrorType {
    /// No error or initial state
    #[default]
    None,
    /// Jito bundle state unknown - DO NOT retry with new blockhash without checking status first.
    /// This occurs on timeouts, server errors, or ambiguous responses where the bundle
    /// may have been processed.
    JitoStateUnknown,
    /// Jito bundle definitively failed - safe to retry with new blockhash.
    /// The bundle was rejected/dropped and was NOT processed.
    JitoBundleFailed,
    /// Transaction failed on-chain - safe to retry with new blockhash.
    TransactionFailed,
    /// Network/connection error - safe to retry with new blockhash.
    NetworkError,
    /// Validation error - should not retry automatically.
    ValidationError,
}

impl LastErrorType {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::None => "none",
            Self::JitoStateUnknown => "jito_state_unknown",
            Self::JitoBundleFailed => "jito_bundle_failed",
            Self::TransactionFailed => "transaction_failed",
            Self::NetworkError => "network_error",
            Self::ValidationError => "validation_error",
        }
    }

    /// Check if this error type requires status verification before retry
    pub fn requires_status_check(&self) -> bool {
        matches!(self, Self::JitoStateUnknown)
    }

    /// Check if this error type is safe to retry with a new blockhash
    pub fn safe_to_retry_new_blockhash(&self) -> bool {
        matches!(
            self,
            Self::None | Self::JitoBundleFailed | Self::TransactionFailed | Self::NetworkError
        )
    }
}

impl std::str::FromStr for LastErrorType {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "none" => Ok(Self::None),
            "jito_state_unknown" => Ok(Self::JitoStateUnknown),
            "jito_bundle_failed" => Ok(Self::JitoBundleFailed),
            "transaction_failed" => Ok(Self::TransactionFailed),
            "network_error" => Ok(Self::NetworkError),
            "validation_error" => Ok(Self::ValidationError),
            _ => Err(format!("Invalid last error type: {}", s)),
        }
    }
}

impl std::fmt::Display for LastErrorType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

/// Transaction status from blockchain query.
/// Used to determine if an original transaction was processed before retrying.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TransactionStatus {
    /// Transaction is confirmed (1+ confirmations)
    Confirmed,
    /// Transaction is finalized (max confirmations)
    Finalized,
    /// Transaction failed with an error
    Failed(String),
}

/// Type of transfer and associated data
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, ToSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum TransferType {
    /// Standard public transfer with visible amount
    Public {
        /// Amount in atomic units
        #[schema(example = 1_000_000_000)]
        amount: u64,
    },
    /// Confidential transfer with zero-knowledge proofs (Token-2022)
    ///
    /// The client must generate all ZK proofs locally and submit the individual
    /// proof components. The server constructs the instruction from these
    /// components, ensuring it maintains full control over what it signs.
    Confidential {
        /// New source decryptable available balance after transfer (Base64 AES ciphertext)
        /// This is the encrypted balance that only the source account holder can decrypt
        #[schema(example = "SGVsbG8gV29ybGQ=")]
        new_decryptable_available_balance: String,

        /// Equality proof (Base64 serialized CiphertextCommitmentEqualityProofData)
        /// Proves that the new balance ciphertext correctly results from subtracting
        /// the transfer amount from the current balance
        #[schema(example = "SGVsbG8gRXF1YWxpdHkgUHJvb2Y=")]
        equality_proof: String,

        /// Ciphertext validity proof (Base64 serialized BatchedGroupedCiphertext3HandlesValidityProofData)
        /// Proves that the transfer amount ciphertexts are correctly constructed for
        /// source, destination, and optional auditor
        #[schema(example = "SGVsbG8gVmFsaWRpdHkgUHJvb2Y=")]
        ciphertext_validity_proof: String,

        /// Range proof (Base64 serialized BatchedRangeProofU128Data)
        /// Proves that the new balance and transfer amount are non-negative
        /// and within valid range
        #[schema(example = "SGVsbG8gUmFuZ2UgUHJvb2Y=")]
        range_proof: String,
    },
}

/// Core transfer request entity
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, ToSchema)]
pub struct TransferRequest {
    /// Unique identifier (UUID)
    #[schema(example = "550e8400-e29b-41d4-a716-446655440000")]
    pub id: String,
    /// Sender wallet address (Base58 Solana address)
    #[schema(example = "HvwC9QSAzwEXkUkwqNNGhfNHoVqXJYfPvPZfQvJmHWcF")]
    pub from_address: String,
    /// Recipient wallet address (Base58 Solana address)
    #[schema(example = "DRpbCBMxVnDK7maPM5tGv6MvB3v1sRMC86PZ8okm21hy")]
    pub to_address: String,

    /// Transfer details (Public vs Confidential)
    pub transfer_details: TransferType,

    /// Optional SPL Token mint address. None means native SOL transfer.
    #[schema(example = "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v")]
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub token_mint: Option<String>,

    /// Compliance check status
    pub compliance_status: ComplianceStatus,
    /// Blockchain submission status
    pub blockchain_status: BlockchainStatus,
    /// Blockchain transaction signature (if submitted)
    #[schema(example = "5eykt4UsFv8P8NJdTREpY1vzqKqZKvdpKuc147dw2N9d")]
    pub blockchain_signature: Option<String>,
    /// Number of retry attempts for blockchain submission
    pub blockchain_retry_count: i32,
    /// Last error message from blockchain submission
    pub blockchain_last_error: Option<String>,
    /// Next scheduled retry time
    pub blockchain_next_retry_at: Option<DateTime<Utc>>,

    // =========================================================================
    // Jito Double Spend Protection Fields
    // =========================================================================
    /// Original transaction signature stored on first submission.
    /// Used to check status before retry when last_error_type is JitoStateUnknown.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub original_tx_signature: Option<String>,

    /// Classification of the last error encountered during blockchain submission.
    /// Determines retry strategy: JitoStateUnknown requires status check before retry.
    #[serde(default)]
    pub last_error_type: LastErrorType,

    /// Blockhash used in the last transaction attempt.
    /// Used to determine if blockhash has expired (>150 slots = safe to retry with new blockhash).
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub blockhash_used: Option<String>,

    /// Creation timestamp
    pub created_at: DateTime<Utc>,
    /// Last update timestamp
    pub updated_at: DateTime<Utc>,
}

impl TransferRequest {
    #[must_use]
    pub fn new(id: String, from_address: String, to_address: String, amount: u64) -> Self {
        let now = Utc::now();
        Self {
            id,
            from_address,
            to_address,
            transfer_details: TransferType::Public { amount },
            token_mint: None,
            compliance_status: ComplianceStatus::Pending,
            blockchain_status: BlockchainStatus::Pending,
            blockchain_signature: None,
            blockchain_retry_count: 0,
            blockchain_last_error: None,
            blockchain_next_retry_at: None,
            // Jito Double Spend Protection fields
            original_tx_signature: None,
            last_error_type: LastErrorType::None,
            blockhash_used: None,
            created_at: now,
            updated_at: now,
        }
    }

    /// Create a new token transfer request
    #[must_use]
    pub fn new_token(
        id: String,
        from_address: String,
        to_address: String,
        amount: u64,
        token_mint: String,
    ) -> Self {
        let mut request = Self::new(id, from_address, to_address, amount);
        request.token_mint = Some(token_mint);
        request
    }

    /// Check if this is an SPL Token transfer
    #[must_use]
    pub fn is_token_transfer(&self) -> bool {
        self.token_mint.is_some()
    }
}

impl Default for TransferRequest {
    fn default() -> Self {
        Self::new(
            "default_id".to_string(),
            "default_from".to_string(),
            "default_to".to_string(),
            0,
        )
    }
}

/// Request to submit a new transfer
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct SubmitTransferRequest {
    /// Sender wallet address (Base58 Solana address)
    #[schema(example = "HvwC9QSAzwEXkUkwqNNGhfNHoVqXJYfPvPZfQvJmHWcF")]
    pub from_address: String,
    /// Recipient wallet address (Base58 Solana address)
    #[schema(example = "DRpbCBMxVnDK7maPM5tGv6MvB3v1sRMC86PZ8okm21hy")]
    pub to_address: String,

    /// Transfer details (Public or Confidential)
    pub transfer_details: TransferType,

    /// Optional SPL Token mint address. If None, this is a native SOL transfer.
    /// If Some, this is an SPL Token transfer for the specified mint.
    #[schema(example = "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v")]
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub token_mint: Option<String>,

    /// Base58-encoded Ed25519 signature proving ownership of from_address.
    /// The message format is: "{from_address}:{to_address}:{amount|confidential}:{token_mint|SOL}"
    #[schema(
        example = "5eykt4UsFv8P8NJdTREpY1vzqKqZKvdpKuc147dw2N9d5eykt4UsFv8P8NJdTREpY1vzqKqZKvdpKuc147dw2N9d"
    )]
    pub signature: String,
}

impl Validate for SubmitTransferRequest {
    fn validate(&self) -> Result<(), validator::ValidationErrors> {
        let mut errors = validator::ValidationErrors::new();

        if self.from_address.is_empty() {
            errors.add(
                "from_address",
                validator::ValidationError::new("From address is required"),
            );
        }
        if self.to_address.is_empty() {
            errors.add(
                "to_address",
                validator::ValidationError::new("To address is required"),
            );
        }

        match &self.transfer_details {
            TransferType::Public { amount } => {
                if *amount == 0 {
                    errors.add(
                        "amount",
                        validator::ValidationError::new("Amount must be greater than 0"),
                    );
                }
            }
            TransferType::Confidential {
                new_decryptable_available_balance,
                equality_proof,
                ciphertext_validity_proof,
                range_proof,
            } => {
                if new_decryptable_available_balance.is_empty() {
                    errors.add(
                        "new_decryptable_available_balance",
                        validator::ValidationError::new(
                            "New decryptable available balance is required for confidential transfers",
                        ),
                    );
                }
                if equality_proof.is_empty() {
                    errors.add(
                        "equality_proof",
                        validator::ValidationError::new(
                            "Equality proof is required for confidential transfers",
                        ),
                    );
                }
                if ciphertext_validity_proof.is_empty() {
                    errors.add(
                        "ciphertext_validity_proof",
                        validator::ValidationError::new(
                            "Ciphertext validity proof is required for confidential transfers",
                        ),
                    );
                }
                if range_proof.is_empty() {
                    errors.add(
                        "range_proof",
                        validator::ValidationError::new(
                            "Range proof is required for confidential transfers",
                        ),
                    );
                }
            }
        }

        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors)
        }
    }
}

impl SubmitTransferRequest {
    /// Verify that the signature is valid for this request.
    /// Returns Ok(()) if valid, or AppError::Authorization if invalid.
    pub fn verify_signature(&self) -> Result<(), AppError> {
        // Construct the deterministic message to verify
        let message = self.create_signing_message();

        // Decode the from_address as a Solana public key (32 bytes)
        let pubkey_bytes = bs58::decode(&self.from_address).into_vec().map_err(|e| {
            AppError::Authorization(format!("Invalid from_address encoding: {}", e))
        })?;

        if pubkey_bytes.len() != 32 {
            return Err(AppError::Authorization(format!(
                "Invalid from_address length: expected 32 bytes, got {}",
                pubkey_bytes.len()
            )));
        }

        let pubkey_array: [u8; 32] = pubkey_bytes
            .try_into()
            .map_err(|_| AppError::Authorization("Invalid from_address format".to_string()))?;

        let verifying_key = VerifyingKey::from_bytes(&pubkey_array)
            .map_err(|e| AppError::Authorization(format!("Invalid public key: {}", e)))?;

        // Decode the signature (64 bytes)
        let sig_bytes = bs58::decode(&self.signature)
            .into_vec()
            .map_err(|e| AppError::Authorization(format!("Invalid signature encoding: {}", e)))?;

        if sig_bytes.len() != 64 {
            return Err(AppError::Authorization(format!(
                "Invalid signature length: expected 64 bytes, got {}",
                sig_bytes.len()
            )));
        }

        let sig_array: [u8; 64] = sig_bytes
            .try_into()
            .map_err(|_| AppError::Authorization("Invalid signature format".to_string()))?;

        let signature = Signature::from_bytes(&sig_array);

        // Verify the signature
        verifying_key
            .verify_strict(&message, &signature)
            .map_err(|e| {
                AppError::Authorization(format!("Signature verification failed: {}", e))
            })?;

        Ok(())
    }

    /// Create the deterministic message for signing.
    /// Format: "{from_address}:{to_address}:{amount|confidential}:{token_mint|SOL}"
    #[must_use]
    pub fn create_signing_message(&self) -> Vec<u8> {
        let amount_part = match &self.transfer_details {
            TransferType::Public { amount } => amount.to_string(),
            TransferType::Confidential { .. } => "confidential".to_string(),
        };
        let mint_part = self.token_mint.as_deref().unwrap_or("SOL");
        format!(
            "{}:{}:{}:{}",
            self.from_address, self.to_address, amount_part, mint_part
        )
        .into_bytes()
    }

    #[must_use]
    pub fn new(from_address: String, to_address: String, amount: u64, signature: String) -> Self {
        Self {
            from_address,
            to_address,
            transfer_details: TransferType::Public { amount },
            token_mint: None,
            signature,
        }
    }

    /// Create a new SPL Token transfer request
    #[must_use]
    pub fn new_token_transfer(
        from_address: String,
        to_address: String,
        amount: u64,
        token_mint: String,
        signature: String,
    ) -> Self {
        Self {
            from_address,
            to_address,
            transfer_details: TransferType::Public { amount },
            token_mint: Some(token_mint),
            signature,
        }
    }

    /// Create a new Confidential transfer request
    #[must_use]
    #[allow(clippy::too_many_arguments)]
    pub fn new_confidential(
        from_address: String,
        to_address: String,
        new_decryptable_available_balance: String,
        equality_proof: String,
        ciphertext_validity_proof: String,
        range_proof: String,
        token_mint: String,
        signature: String,
    ) -> Self {
        Self {
            from_address,
            to_address,
            transfer_details: TransferType::Confidential {
                new_decryptable_available_balance,
                equality_proof,
                ciphertext_validity_proof,
                range_proof,
            },
            token_mint: Some(token_mint),
            signature,
        }
    }

    /// Check if this is an SPL Token transfer
    #[must_use]
    pub fn is_token_transfer(&self) -> bool {
        self.token_mint.is_some()
    }
}

/// Pagination parameters for list requests
#[derive(Debug, Clone, Serialize, Deserialize, Validate, ToSchema)]
pub struct PaginationParams {
    /// Maximum number of items to return (1-100, default: 20)
    #[validate(range(min = 1, max = 100, message = "Limit must be between 1 and 100"))]
    #[serde(default = "default_limit")]
    #[schema(example = 20)]
    pub limit: i64,
    /// Cursor for pagination (ID to start after)
    #[schema(example = "uuid-string")]
    pub cursor: Option<String>,
}

fn default_limit() -> i64 {
    20
}

impl Default for PaginationParams {
    fn default() -> Self {
        Self {
            limit: default_limit(),
            cursor: None,
        }
    }
}

/// Paginated response wrapper
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct PaginatedResponse<T: ToSchema> {
    /// List of items
    pub items: Vec<T>,
    /// Cursor for next page (null if no more items)
    #[schema(example = "uuid-string")]
    pub next_cursor: Option<String>,
    /// Whether more items exist
    pub has_more: bool,
}

impl<T: ToSchema> PaginatedResponse<T> {
    pub fn new(items: Vec<T>, next_cursor: Option<String>, has_more: bool) -> Self {
        Self {
            items,
            next_cursor,
            has_more,
        }
    }

    pub fn empty() -> Self {
        Self {
            items: Vec::new(),
            next_cursor: None,
            has_more: false,
        }
    }
}

/// Health status enum
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum HealthStatus {
    /// All systems operational
    Healthy,
    /// Some systems degraded but functional
    Degraded,
    /// Critical systems unavailable
    Unhealthy,
}

/// Health check response
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct HealthResponse {
    /// Overall system status
    pub status: HealthStatus,
    /// Database health status
    pub database: HealthStatus,
    /// Blockchain client health status
    pub blockchain: HealthStatus,
    /// Current server timestamp
    pub timestamp: DateTime<Utc>,
    /// Application version
    #[schema(example = "0.3.0")]
    pub version: String,
}

impl HealthResponse {
    #[must_use]
    pub fn new(database: HealthStatus, blockchain: HealthStatus) -> Self {
        let status = match (&database, &blockchain) {
            (HealthStatus::Healthy, HealthStatus::Healthy) => HealthStatus::Healthy,
            (HealthStatus::Unhealthy, _) | (_, HealthStatus::Unhealthy) => HealthStatus::Unhealthy,
            _ => HealthStatus::Degraded,
        };
        Self {
            status,
            database,
            blockchain,
            timestamp: Utc::now(),
            version: env!("CARGO_PKG_VERSION").to_string(),
        }
    }
}

/// Error response structure
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ErrorResponse {
    /// Error details
    pub error: ErrorDetail,
}

/// Error detail structure
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ErrorDetail {
    /// Error type identifier
    #[schema(example = "validation_error")]
    pub r#type: String,
    /// Human-readable error message
    #[schema(example = "Name must be between 1 and 255 characters")]
    pub message: String,
}

/// Rate limit exceeded response
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct RateLimitResponse {
    /// Error details
    pub error: ErrorDetail,
    /// Seconds until rate limit resets
    #[schema(example = 60)]
    pub retry_after: u64,
}

// ============================================================================
// Risk Check Types (Pre-flight Compliance Screening)
// ============================================================================

/// Request to check wallet risk status
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct RiskCheckRequest {
    /// Wallet address to check (Base58 Solana address)
    #[schema(example = "HvwC9QSAzwEXkUkwqNNGhfNHoVqXJYfPvPZfQvJmHWcF")]
    pub address: String,
}

/// Cached wallet risk profile from database
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WalletRiskProfile {
    pub address: String,
    pub risk_score: Option<i32>,
    pub risk_level: Option<String>,
    pub reasoning: Option<String>,
    pub has_sanctioned_assets: bool,
    pub helius_assets_checked: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Result of a wallet risk check (polymorphic response)
///
/// This enum differentiates between:
/// - **Blocked**: Wallet found in internal blocklist (no external API calls made)
/// - **Analyzed**: Wallet checked against Range Protocol and Helius DAS
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum RiskCheckResult {
    /// Wallet is blocked by internal blocklist.
    /// No risk score is returned as we skip external checks to save resources.
    Blocked {
        /// The wallet address that was checked
        #[schema(example = "4oS78GPe66RqBduuAeiMFANf27FpmgXNwokZ3ocN4z1B")]
        address: String,
        /// Reason for blocking from the internal blocklist
        #[schema(example = "Internal Security Alert: Address linked to Phishing Scam")]
        reason: String,
    },
    /// Wallet was analyzed (clean or warning level).
    /// Contains aggregated data from Range Protocol and Helius DAS.
    Analyzed {
        /// The wallet address that was checked
        #[schema(example = "HvwC9QSAzwEXkUkwqNNGhfNHoVqXJYfPvPZfQvJmHWcF")]
        address: String,
        /// Risk score from Range Protocol (1-10, higher = more risky)
        #[schema(example = 2)]
        risk_score: i32,
        /// Risk level description from Range Protocol
        #[schema(example = "Low risk")]
        risk_level: String,
        /// Reasoning from Range Protocol
        #[schema(example = "3 hops from nearest flagged address")]
        reasoning: String,
        /// Whether the wallet holds sanctioned NFTs/assets (from Helius DAS)
        #[schema(example = false)]
        has_sanctioned_assets: bool,
        /// Whether Helius DAS check was actually performed.
        /// If false, `has_sanctioned_assets` is a default value (check skipped).
        #[schema(example = true)]
        helius_assets_checked: bool,
        /// Whether this result came from cache
        #[schema(example = false)]
        from_cache: bool,
        /// Timestamp when the risk check was performed
        checked_at: DateTime<Utc>,
    },
}

/// Single transaction from Helius webhook (Enhanced Transaction format)
/// Reference: <https://docs.helius.dev/webhooks-and-websockets/webhooks>
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HeliusTransaction {
    /// Transaction type (e.g., "TRANSFER", "NFT_SALE", "UNKNOWN")
    #[serde(rename = "type")]
    pub transaction_type: String,
    /// Transaction signature (base58)
    pub signature: String,
    /// Transaction error (null if successful, object/string if failed)
    #[serde(default)]
    pub transaction_error: Option<serde_json::Value>,
    /// Source program (e.g., "SYSTEM_PROGRAM")
    #[serde(default)]
    pub source: String,
}

// ============================================================================
// QuickNode Webhook Types
// ============================================================================

/// QuickNode webhook payload wrapper
/// Reference: <https://www.quicknode.com/docs/webhooks>
///
/// QuickNode webhooks can deliver multiple events in a single POST payload.
/// The payload is an array of events, NOT a single event object.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum QuickNodeWebhookPayload {
    /// Array of events (most common)
    Events(Vec<QuickNodeWebhookEvent>),
    /// Single event (for backwards compatibility)
    Single(QuickNodeWebhookEvent),
}

impl QuickNodeWebhookPayload {
    /// Convert to a vector of events for uniform processing
    pub fn into_events(self) -> Vec<QuickNodeWebhookEvent> {
        match self {
            QuickNodeWebhookPayload::Events(events) => events,
            QuickNodeWebhookPayload::Single(event) => vec![event],
        }
    }
}

/// Single event from QuickNode Streams/Webhooks
///
/// QuickNode Streams can be configured with various filters and templates.
/// For transaction confirmation, we use the "Solana Transaction" template
/// which provides transaction metadata including signature and status.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QuickNodeWebhookEvent {
    /// Transaction signature (base58)
    pub signature: String,

    /// Slot number where the transaction was processed
    #[serde(default)]
    pub slot: Option<u64>,

    /// Block time (Unix timestamp) when the transaction was processed
    #[serde(default)]
    pub block_time: Option<i64>,

    /// Transaction error (null if successful)
    /// Can be a string error message or structured error object
    #[serde(default)]
    pub err: Option<serde_json::Value>,

    /// Transaction metadata (varies based on Stream template)
    #[serde(default)]
    pub meta: Option<QuickNodeTransactionMeta>,
}

impl QuickNodeWebhookEvent {
    /// Check if the transaction was successful
    pub fn is_success(&self) -> bool {
        self.err.is_none() && self.meta.as_ref().is_none_or(|m| m.err.is_none())
    }

    /// Get the error message if the transaction failed
    pub fn error_message(&self) -> Option<String> {
        if let Some(err) = &self.err {
            return Some(err.to_string());
        }
        if let Some(meta) = &self.meta
            && let Some(err) = &meta.err
        {
            return Some(err.to_string());
        }
        None
    }
}

/// Transaction metadata from QuickNode webhook
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QuickNodeTransactionMeta {
    /// Transaction error from meta (null if successful)
    #[serde(default)]
    pub err: Option<serde_json::Value>,

    /// Fee in lamports
    #[serde(default)]
    pub fee: Option<u64>,

    /// Pre-transaction balances
    #[serde(default)]
    pub pre_balances: Vec<u64>,

    /// Post-transaction balances
    #[serde(default)]
    pub post_balances: Vec<u64>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    #[test]
    fn test_blockchain_status_display_and_parsing() {
        let statuses = vec![
            (BlockchainStatus::Pending, "pending"),
            (BlockchainStatus::PendingSubmission, "pending_submission"),
            (BlockchainStatus::Submitted, "submitted"),
            (BlockchainStatus::Confirmed, "confirmed"),
            (BlockchainStatus::Failed, "failed"),
        ];

        for (status, string) in statuses {
            assert_eq!(status.as_str(), string);
            assert_eq!(status.to_string(), string);
            assert_eq!(BlockchainStatus::from_str(string).unwrap(), status);
        }

        assert!(BlockchainStatus::from_str("invalid").is_err());
    }

    #[test]
    fn test_compliance_status_display_and_parsing() {
        let statuses = vec![
            (ComplianceStatus::Pending, "pending"),
            (ComplianceStatus::Approved, "approved"),
            (ComplianceStatus::Rejected, "rejected"),
        ];

        for (status, string) in statuses {
            assert_eq!(status.as_str(), string);
            assert_eq!(status.to_string(), string);
            assert_eq!(ComplianceStatus::from_str(string).unwrap(), status);
        }

        assert!(ComplianceStatus::from_str("invalid").is_err());
    }

    #[test]
    fn test_submit_transfer_request_validation() {
        // Valid request (1 SOL in lamports)
        let req = SubmitTransferRequest::new(
            "From".to_string(),
            "To".to_string(),
            1_000_000_000,
            "sig".to_string(),
        );
        assert!(req.validate().is_ok());

        // Invalid From (empty)
        let req = SubmitTransferRequest::new(
            "".to_string(),
            "To".to_string(),
            1_000_000_000,
            "sig".to_string(),
        );
        assert!(req.validate().is_err());

        // Invalid To (empty)
        let req = SubmitTransferRequest::new(
            "From".to_string(),
            "".to_string(),
            1_000_000_000,
            "sig".to_string(),
        );
        assert!(req.validate().is_err());

        // Invalid Amount (zero)
        let req =
            SubmitTransferRequest::new("From".to_string(), "To".to_string(), 0, "sig".to_string());
        assert!(req.validate().is_err());

        // Valid Confidential Request
        let req = SubmitTransferRequest::new_confidential(
            "From".to_string(),
            "To".to_string(),
            "balance".to_string(),
            "equality".to_string(),
            "validity".to_string(),
            "range".to_string(),
            "mint".to_string(),
            "sig".to_string(),
        );
        assert!(req.validate().is_ok());

        // Invalid Confidential (empty equality proof)
        let req = SubmitTransferRequest::new_confidential(
            "From".to_string(),
            "To".to_string(),
            "balance".to_string(),
            "".to_string(),
            "validity".to_string(),
            "range".to_string(),
            "mint".to_string(),
            "sig".to_string(),
        );
        assert!(req.validate().is_err());
    }

    #[test]
    fn test_transfer_request_initialization_defaults() {
        let req = TransferRequest::new(
            "id_123".to_string(),
            "from_123".to_string(),
            "to_123".to_string(),
            10_500_000_000, // 10.5 SOL in lamports
        );

        assert_eq!(req.compliance_status, ComplianceStatus::Pending);
        assert_eq!(req.blockchain_status, BlockchainStatus::Pending);
        assert!(req.blockchain_signature.is_none());
        assert_eq!(req.blockchain_retry_count, 0);
        assert!(req.blockchain_last_error.is_none());
        assert!(req.blockchain_next_retry_at.is_none());
        assert_eq!(
            req.transfer_details,
            TransferType::Public {
                amount: 10_500_000_000
            }
        );
    }

    #[test]
    fn test_transfer_request_serialization_roundtrip() {
        let req = TransferRequest::new(
            "tr_123".to_string(),
            "from_abc".to_string(),
            "to_xyz".to_string(),
            5_000_000_000, // 5 SOL in lamports
        );

        let json = serde_json::to_string(&req).unwrap();
        let deserialized: TransferRequest = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.id, "tr_123");
        assert_eq!(deserialized.from_address, "from_abc");
        assert_eq!(deserialized.to_address, "to_xyz");
        assert_eq!(
            deserialized.transfer_details,
            TransferType::Public {
                amount: 5_000_000_000
            }
        );
    }

    // ========================================================================
    // QuickNode Webhook Types Tests
    // ========================================================================

    #[test]
    fn test_quicknode_webhook_payload_batch_parsing() {
        // QuickNode webhooks can deliver an array of events (most common case)
        let json = r#"[
            {"signature": "sig1", "slot": 12345, "err": null},
            {"signature": "sig2", "slot": 12346, "err": null},
            {"signature": "sig3", "slot": 12347, "err": {"InstructionError": [0, "Custom error"]}}
        ]"#;

        let payload: QuickNodeWebhookPayload = serde_json::from_str(json).unwrap();
        let events = payload.into_events();

        assert_eq!(events.len(), 3);
        assert_eq!(events[0].signature, "sig1");
        assert_eq!(events[1].signature, "sig2");
        assert_eq!(events[2].signature, "sig3");
    }

    #[test]
    fn test_quicknode_webhook_payload_single_parsing() {
        // Single event (less common but supported)
        let json = r#"{"signature": "single_sig", "slot": 99999}"#;

        let payload: QuickNodeWebhookPayload = serde_json::from_str(json).unwrap();
        let events = payload.into_events();

        assert_eq!(events.len(), 1);
        assert_eq!(events[0].signature, "single_sig");
        assert_eq!(events[0].slot, Some(99999));
    }

    #[test]
    fn test_quicknode_webhook_event_success_detection() {
        // Successful transaction (no error)
        let event = QuickNodeWebhookEvent {
            signature: "success_sig".to_string(),
            slot: Some(12345),
            block_time: Some(1700000000),
            err: None,
            meta: None,
        };
        assert!(event.is_success());
        assert!(event.error_message().is_none());

        // Failed transaction (err field set)
        let event = QuickNodeWebhookEvent {
            signature: "failed_sig".to_string(),
            slot: Some(12345),
            block_time: Some(1700000000),
            err: Some(serde_json::json!({"InstructionError": [0, "Custom error"]})),
            meta: None,
        };
        assert!(!event.is_success());
        assert!(event.error_message().is_some());
    }

    #[test]
    fn test_quicknode_webhook_event_meta_error() {
        // Failed transaction (error in meta.err)
        let meta = QuickNodeTransactionMeta {
            err: Some(serde_json::json!("InsufficientFunds")),
            fee: Some(5000),
            pre_balances: vec![1000000, 0],
            post_balances: vec![995000, 0],
        };

        let event = QuickNodeWebhookEvent {
            signature: "meta_error_sig".to_string(),
            slot: Some(12345),
            block_time: None,
            err: None,
            meta: Some(meta),
        };
        assert!(!event.is_success());
        assert!(event.error_message().is_some());
    }
}

//! Domain types with validation support.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use validator::Validate;

/// Status of blockchain submission for a transfer
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum BlockchainStatus {
    /// Initial state, not yet processed
    #[default]
    Pending,
    /// Waiting to be submitted to blockchain
    PendingSubmission,
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
    /// Amount of SOL to transfer
    #[schema(example = 0.5)]
    pub amount_sol: f64,
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
    /// Creation timestamp
    pub created_at: DateTime<Utc>,
    /// Last update timestamp
    pub updated_at: DateTime<Utc>,
}

impl TransferRequest {
    #[must_use]
    pub fn new(id: String, from_address: String, to_address: String, amount_sol: f64) -> Self {
        let now = Utc::now();
        Self {
            id,
            from_address,
            to_address,
            amount_sol,
            compliance_status: ComplianceStatus::Pending,
            blockchain_status: BlockchainStatus::Pending,
            blockchain_signature: None,
            blockchain_retry_count: 0,
            blockchain_last_error: None,
            blockchain_next_retry_at: None,
            created_at: now,
            updated_at: now,
        }
    }
}

impl Default for TransferRequest {
    fn default() -> Self {
        Self::new(
            "default_id".to_string(),
            "default_from".to_string(),
            "default_to".to_string(),
            0.0,
        )
    }
}

/// Request to submit a new transfer
#[derive(Debug, Clone, Serialize, Deserialize, Validate, ToSchema)]
pub struct SubmitTransferRequest {
    /// Sender wallet address (Base58 Solana address)
    #[validate(length(min = 1, message = "From address is required"))]
    #[schema(example = "HvwC9QSAzwEXkUkwqNNGhfNHoVqXJYfPvPZfQvJmHWcF")]
    pub from_address: String,
    /// Recipient wallet address (Base58 Solana address)
    #[validate(length(min = 1, message = "To address is required"))]
    #[schema(example = "DRpbCBMxVnDK7maPM5tGv6MvB3v1sRMC86PZ8okm21hy")]
    pub to_address: String,
    /// Amount of SOL to transfer
    #[validate(range(min = 0.000000001, message = "Amount must be greater than 0"))]
    #[schema(example = 0.5)]
    pub amount_sol: f64,
}

impl SubmitTransferRequest {
    #[must_use]
    pub fn new(from_address: String, to_address: String, amount_sol: f64) -> Self {
        Self {
            from_address,
            to_address,
            amount_sol,
        }
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
        // Valid request
        let req = SubmitTransferRequest::new("From".to_string(), "To".to_string(), 1.0);
        assert!(req.validate().is_ok());

        // Invalid From (empty)
        let req = SubmitTransferRequest::new("".to_string(), "To".to_string(), 1.0);
        assert!(req.validate().is_err());

        // Invalid To (empty)
        let req = SubmitTransferRequest::new("From".to_string(), "".to_string(), 1.0);
        assert!(req.validate().is_err());

        // Invalid Amount (zero)
        let req = SubmitTransferRequest::new("From".to_string(), "To".to_string(), 0.0);
        assert!(req.validate().is_err());
    }

    #[test]
    fn test_transfer_request_initialization_defaults() {
        let req = TransferRequest::new(
            "id_123".to_string(),
            "from_123".to_string(),
            "to_123".to_string(),
            10.5,
        );

        assert_eq!(req.compliance_status, ComplianceStatus::Pending);
        assert_eq!(req.blockchain_status, BlockchainStatus::Pending);
        assert!(req.blockchain_signature.is_none());
        assert_eq!(req.blockchain_retry_count, 0);
        assert!(req.blockchain_last_error.is_none());
        assert!(req.blockchain_next_retry_at.is_none());
    }

    #[test]
    fn test_transfer_request_serialization_roundtrip() {
        let req = TransferRequest::new(
            "tr_123".to_string(),
            "from_abc".to_string(),
            "to_xyz".to_string(),
            5.0,
        );

        let json = serde_json::to_string(&req).unwrap();
        let deserialized: TransferRequest = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.id, "tr_123");
        assert_eq!(deserialized.from_address, "from_abc");
        assert_eq!(deserialized.to_address, "to_xyz");
        assert_eq!(deserialized.amount_sol, 5.0);
    }
}

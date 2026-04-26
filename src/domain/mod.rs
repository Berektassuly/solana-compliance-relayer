//! Domain layer containing core business types, traits, and error definitions.

pub mod error;
pub mod traits;
pub mod types;

pub use error::{
    AppError, BlockchainError, ConfigError, DatabaseError, ExternalServiceError, ValidationError,
};
pub use traits::{BlockchainClient, ComplianceProvider, DatabaseClient};
pub use types::{
    AuditAmount, AuditAssetType, AuditFinalDecision, BlockchainStatus, CheckoutSession,
    CheckoutSessionStatus, CheckoutTransferSubmissionResponse, ComplianceStatus,
    CreateCheckoutSessionRequest, ErrorDetail, ErrorResponse, HealthResponse, HealthStatus,
    HeliusTransaction, InternalBlocklistHit, LastErrorType, PaginatedResponse, PaginationParams,
    PrivateSubmissionAuditMetadata, QuickNodeTransactionMeta, QuickNodeWebhookEvent,
    QuickNodeWebhookPayload, RateLimitResponse, RiskCheckRequest, RiskCheckResult,
    SubmitTransferRequest, TransactionStatus, TransferAuditReport, TransferRequest, TransferType,
    WalletRiskProfile,
};

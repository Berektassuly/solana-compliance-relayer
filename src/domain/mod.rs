//! Domain layer containing core business types, traits, and error definitions.

pub mod error;
pub mod traits;
pub mod types;

pub use error::{
    AppError, BlockchainError, ConfigError, DatabaseError, ExternalServiceError, ValidationError,
};
pub use traits::{BlockchainClient, DatabaseClient};
pub use types::{
    BlockchainStatus, ComplianceStatus, ErrorDetail, ErrorResponse, HealthResponse, HealthStatus,
    PaginatedResponse, PaginationParams, RateLimitResponse, SubmitTransferRequest, TransferRequest,
};

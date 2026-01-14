//! Domain layer containing core business types, traits, and error definitions.

pub mod error;
pub mod traits;
pub mod types;

pub use error::{
    AppError, BlockchainError, ConfigError, DatabaseError, ExternalServiceError, ValidationError,
};
pub use traits::{BlockchainClient, ComplianceProvider, DatabaseClient};
pub use types::{
    BlockchainStatus, ComplianceStatus, ErrorDetail, ErrorResponse, HealthResponse, HealthStatus,
    HeliusTransaction, PaginatedResponse, PaginationParams, RateLimitResponse,
    SubmitTransferRequest, TransferRequest,
};

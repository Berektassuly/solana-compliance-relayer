//! HTTP request handlers with OpenAPI documentation.

use std::sync::Arc;

use axum::{
    Json,
    extract::{Path, Query, State},
    http::StatusCode,
    response::IntoResponse,
};
use tracing::error;
use utoipa::OpenApi;

use crate::app::AppState;
use crate::domain::{
    AppError, BlockchainError, DatabaseError, ErrorDetail, ErrorResponse, ExternalServiceError,
    HealthResponse, HealthStatus, PaginatedResponse, PaginationParams, RateLimitResponse,
    SubmitTransferRequest, TransferRequest,
};

/// OpenAPI documentation structure
#[derive(OpenApi)]
#[openapi(
    info(
        title = "Solana Compliance Relayer API",
        version = "0.3.0",
        description = "API for submitting and tracking compliant Solana transfers",
        contact(
            name = "API Support",
            email = "support@example.com"
        ),
        license(
            name = "MIT"
        )
    ),
    paths(
        submit_transfer_handler,
        list_transfer_requests_handler,
        get_transfer_request_handler,
        retry_blockchain_handler,
        health_check_handler,
        liveness_handler,
        readiness_handler,
    ),
    components(
        schemas(
            TransferRequest,
            SubmitTransferRequest,
            crate::domain::ComplianceStatus,
            crate::domain::BlockchainStatus,
            PaginationParams,
            PaginatedResponse<TransferRequest>,
            HealthResponse,
            HealthStatus,
            ErrorResponse,
            ErrorDetail,
            RateLimitResponse,
        )
    ),
    tags(
        (name = "transfers", description = "Transfer request management endpoints"),
        (name = "health", description = "Health check endpoints")
    )
)]
pub struct ApiDoc;

/// Submit a new transfer request
///
/// Submits a transfer for compliance screening. If the recipient address
/// is flagged by the compliance provider, the transfer will be rejected.
#[utoipa::path(
    post,
    path = "/transfer-requests",
    tag = "transfers",
    request_body = SubmitTransferRequest,
    responses(
        (status = 200, description = "Transfer request accepted (check compliance_status for approval)", body = TransferRequest),
        (status = 400, description = "Validation error - invalid request format", body = ErrorResponse),
        (status = 429, description = "Rate limit exceeded", body = RateLimitResponse),
        (status = 500, description = "Internal server error", body = ErrorResponse),
        (status = 503, description = "Service unavailable", body = ErrorResponse)
    )
)]
pub async fn submit_transfer_handler(
    State(state): State<Arc<AppState>>,
    Json(payload): Json<SubmitTransferRequest>,
) -> Result<Json<TransferRequest>, AppError> {
    let request = state.service.submit_transfer(&payload).await?;
    Ok(Json(request))
}

/// List transfer requests with pagination
#[utoipa::path(
    get,
    path = "/transfer-requests",
    tag = "transfers",
    params(
        ("limit" = Option<i64>, Query, description = "Maximum number of requests to return (1-100, default: 20)"),
        ("cursor" = Option<String>, Query, description = "Cursor for pagination (request ID to start after)")
    ),
    responses(
        (status = 200, description = "List of transfer requests", body = PaginatedResponse<TransferRequest>),
        (status = 400, description = "Invalid pagination parameters", body = ErrorResponse),
        (status = 429, description = "Rate limit exceeded", body = RateLimitResponse),
        (status = 500, description = "Internal server error", body = ErrorResponse)
    )
)]
pub async fn list_transfer_requests_handler(
    State(state): State<Arc<AppState>>,
    Query(params): Query<PaginationParams>,
) -> Result<Json<PaginatedResponse<TransferRequest>>, AppError> {
    // Validate limit
    let limit = params.limit.clamp(1, 100);
    let requests = state
        .service
        .list_transfer_requests(limit, params.cursor.as_deref())
        .await?;
    Ok(Json(requests))
}

/// Get a single transfer request by ID
#[utoipa::path(
    get,
    path = "/transfer-requests/{id}",
    tag = "transfers",
    params(
        ("id" = String, Path, description = "Transfer Request ID")
    ),
    responses(
        (status = 200, description = "Transfer request found", body = TransferRequest),
        (status = 404, description = "Request not found", body = ErrorResponse),
        (status = 429, description = "Rate limit exceeded", body = RateLimitResponse),
        (status = 500, description = "Internal server error", body = ErrorResponse)
    )
)]
pub async fn get_transfer_request_handler(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<TransferRequest>, AppError> {
    let request = state
        .service
        .get_transfer_request(&id)
        .await?
        .ok_or(AppError::Database(DatabaseError::NotFound(id)))?;
    Ok(Json(request))
}

/// Retry blockchain submission for a transfer request
#[utoipa::path(
    post,
    path = "/transfer-requests/{id}/retry",
    tag = "transfers",
    params(
        ("id" = String, Path, description = "Transfer Request ID")
    ),
    responses(
        (status = 200, description = "Retry successful", body = TransferRequest),
        (status = 400, description = "Request not eligible for retry", body = ErrorResponse),
        (status = 404, description = "Request not found", body = ErrorResponse),
        (status = 429, description = "Rate limit exceeded", body = RateLimitResponse),
        (status = 500, description = "Internal server error", body = ErrorResponse),
        (status = 503, description = "Blockchain unavailable", body = ErrorResponse)
    )
)]
pub async fn retry_blockchain_handler(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<TransferRequest>, AppError> {
    let request = state.service.retry_blockchain_submission(&id).await?;
    Ok(Json(request))
}

/// Detailed health check
#[utoipa::path(
    get,
    path = "/health",
    tag = "health",
    responses(
        (status = 200, description = "Health status", body = HealthResponse)
    )
)]
pub async fn health_check_handler(State(state): State<Arc<AppState>>) -> Json<HealthResponse> {
    let health = state.service.health_check().await;
    Json(health)
}

/// Kubernetes liveness probe
#[utoipa::path(
    get,
    path = "/health/live",
    tag = "health",
    responses(
        (status = 200, description = "Application is alive")
    )
)]
pub async fn liveness_handler() -> StatusCode {
    StatusCode::OK
}

/// Kubernetes readiness probe
#[utoipa::path(
    get,
    path = "/health/ready",
    tag = "health",
    responses(
        (status = 200, description = "Application is ready to serve traffic"),
        (status = 503, description = "Application is not ready")
    )
)]
pub async fn readiness_handler(State(state): State<Arc<AppState>>) -> StatusCode {
    let health = state.service.health_check().await;
    match health.status {
        HealthStatus::Healthy | HealthStatus::Degraded => StatusCode::OK,
        HealthStatus::Unhealthy => StatusCode::SERVICE_UNAVAILABLE,
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> axum::response::Response {
        let (status, error_type, message) = match &self {
            AppError::Database(db_err) => match db_err {
                DatabaseError::Connection(_) => (
                    StatusCode::SERVICE_UNAVAILABLE,
                    "database_error",
                    self.to_string(),
                ),
                DatabaseError::NotFound(_) => {
                    (StatusCode::NOT_FOUND, "not_found", self.to_string())
                }
                DatabaseError::Duplicate(_) => {
                    (StatusCode::CONFLICT, "duplicate", self.to_string())
                }
                _ => (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "database_error",
                    self.to_string(),
                ),
            },
            AppError::Blockchain(bc_err) => match bc_err {
                BlockchainError::Connection(_) => (
                    StatusCode::SERVICE_UNAVAILABLE,
                    "blockchain_error",
                    self.to_string(),
                ),
                BlockchainError::InsufficientFunds => (
                    StatusCode::PAYMENT_REQUIRED,
                    "insufficient_funds",
                    self.to_string(),
                ),
                BlockchainError::Timeout(_) => {
                    (StatusCode::GATEWAY_TIMEOUT, "timeout", self.to_string())
                }
                _ => (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "blockchain_error",
                    self.to_string(),
                ),
            },
            AppError::ExternalService(ext_err) => match ext_err {
                ExternalServiceError::Unavailable(_) => (
                    StatusCode::BAD_GATEWAY,
                    "external_service_error",
                    self.to_string(),
                ),
                ExternalServiceError::Timeout(_) => {
                    (StatusCode::GATEWAY_TIMEOUT, "timeout", self.to_string())
                }
                ExternalServiceError::RateLimited(_) => (
                    StatusCode::TOO_MANY_REQUESTS,
                    "rate_limited",
                    self.to_string(),
                ),
                _ => (
                    StatusCode::BAD_GATEWAY,
                    "external_service_error",
                    self.to_string(),
                ),
            },
            AppError::Config(_) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                "configuration_error",
                self.to_string(),
            ),
            AppError::Validation(_) => (
                StatusCode::BAD_REQUEST,
                "validation_error",
                self.to_string(),
            ),
            AppError::Authentication(_) => (
                StatusCode::UNAUTHORIZED,
                "authentication_error",
                self.to_string(),
            ),
            AppError::Authorization(_) => (
                StatusCode::FORBIDDEN,
                "authorization_error",
                self.to_string(),
            ),
            AppError::Serialization(_) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                "serialization_error",
                self.to_string(),
            ),
            AppError::Deserialization(_) => (
                StatusCode::BAD_REQUEST,
                "deserialization_error",
                self.to_string(),
            ),
            AppError::Internal(_) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal_error",
                self.to_string(),
            ),
            AppError::NotSupported(_) => (
                StatusCode::NOT_IMPLEMENTED,
                "not_supported",
                self.to_string(),
            ),
            AppError::RateLimited => (
                StatusCode::TOO_MANY_REQUESTS,
                "rate_limited",
                "Rate limit exceeded".to_string(),
            ),
        };

        if status.is_server_error() {
            error!(error_type = %error_type, message = %message, "Server error");
        }

        let body = Json(ErrorResponse {
            error: ErrorDetail {
                r#type: error_type.to_string(),
                message,
            },
        });

        (status, body).into_response()
    }
}

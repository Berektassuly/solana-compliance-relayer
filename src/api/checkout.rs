//! Merchant checkout and virtual-card funding API handlers.

use std::sync::Arc;

use axum::{
    Json,
    extract::{Path, State},
};

use crate::app::AppState;
use crate::domain::{
    AppError, CheckoutSession, CheckoutTransferSubmissionResponse, CreateCheckoutSessionRequest,
    DatabaseError, SubmitTransferRequest,
};

/// Create a checkout session for merchant checkout, remittance, or card funding.
#[utoipa::path(
    post,
    path = "/checkout/sessions",
    tag = "checkout",
    request_body = CreateCheckoutSessionRequest,
    responses(
        (status = 200, description = "Checkout session created", body = CheckoutSession),
        (status = 400, description = "Invalid checkout session", body = crate::domain::ErrorResponse),
        (status = 500, description = "Internal server error", body = crate::domain::ErrorResponse)
    )
)]
pub async fn create_checkout_session_handler(
    State(state): State<Arc<AppState>>,
    Json(payload): Json<CreateCheckoutSessionRequest>,
) -> Result<Json<CheckoutSession>, AppError> {
    let session = state.service.create_checkout_session(&payload).await?;
    Ok(Json(session))
}

/// Get a checkout session by ID.
#[utoipa::path(
    get,
    path = "/checkout/sessions/{id}",
    tag = "checkout",
    params(
        ("id" = String, Path, description = "Checkout session ID")
    ),
    responses(
        (status = 200, description = "Checkout session found", body = CheckoutSession),
        (status = 404, description = "Checkout session not found", body = crate::domain::ErrorResponse),
        (status = 500, description = "Internal server error", body = crate::domain::ErrorResponse)
    )
)]
pub async fn get_checkout_session_handler(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<CheckoutSession>, AppError> {
    let session = state
        .service
        .get_checkout_session(&id)
        .await?
        .ok_or(AppError::Database(DatabaseError::NotFound(id)))?;
    Ok(Json(session))
}

/// Submit an existing signed transfer payload to a checkout session.
#[utoipa::path(
    post,
    path = "/checkout/sessions/{id}/submit-transfer",
    tag = "checkout",
    request_body = SubmitTransferRequest,
    params(
        ("id" = String, Path, description = "Checkout session ID")
    ),
    responses(
        (status = 200, description = "Transfer linked to checkout session", body = CheckoutTransferSubmissionResponse),
        (status = 400, description = "Transfer does not match checkout session", body = crate::domain::ErrorResponse),
        (status = 404, description = "Checkout session not found", body = crate::domain::ErrorResponse),
        (status = 500, description = "Internal server error", body = crate::domain::ErrorResponse),
        (status = 503, description = "External dependency unavailable", body = crate::domain::ErrorResponse)
    )
)]
pub async fn submit_checkout_transfer_handler(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(payload): Json<SubmitTransferRequest>,
) -> Result<Json<CheckoutTransferSubmissionResponse>, AppError> {
    let response = state
        .service
        .submit_checkout_transfer(&id, &payload)
        .await?;
    Ok(Json(response))
}

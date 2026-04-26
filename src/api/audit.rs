//! Compliance audit report API handlers.

use std::sync::Arc;

use axum::{
    Json,
    extract::{Path, State},
};

use crate::app::AppState;
use crate::domain::{AppError, DatabaseError, TransferAuditReport};

/// Get a concise compliance and settlement audit report for a transfer.
#[utoipa::path(
    get,
    path = "/transfer-requests/{id}/audit-report",
    tag = "transfers",
    params(
        ("id" = String, Path, description = "Transfer Request ID")
    ),
    responses(
        (status = 200, description = "Transfer audit report", body = TransferAuditReport),
        (status = 404, description = "Transfer request not found", body = crate::domain::ErrorResponse),
        (status = 500, description = "Internal server error", body = crate::domain::ErrorResponse)
    )
)]
pub async fn get_transfer_audit_report_handler(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<TransferAuditReport>, AppError> {
    let report = state
        .service
        .get_transfer_audit_report(&id)
        .await?
        .ok_or(AppError::Database(DatabaseError::NotFound(id)))?;
    Ok(Json(report))
}

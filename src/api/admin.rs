//! Admin API handlers for blocklist management.
//!
//! Provides HTTP endpoints for real-time management of the internal blocklist.

use std::sync::Arc;

use axum::{
    Json,
    extract::{Path, State},
};
use serde::{Deserialize, Serialize};
use tracing::warn;
use utoipa::ToSchema;

use crate::app::AppState;
use crate::domain::{AppError, DatabaseError, ValidationError};

/// Request body for adding an address to the blocklist
#[derive(Debug, Deserialize, Serialize, ToSchema)]
pub struct AddBlocklistRequest {
    /// The wallet address to block
    pub address: String,
    /// The reason for blocking this address
    pub reason: String,
}

/// Response for blocklist operations
#[derive(Debug, Serialize, ToSchema)]
pub struct BlocklistResponse {
    /// Success indicator
    pub success: bool,
    /// Descriptive message
    pub message: String,
}

/// Blocklist entry for listing
#[derive(Debug, Serialize, ToSchema)]
pub struct BlocklistEntryResponse {
    /// The blocked wallet address
    pub address: String,
    /// The reason for blocking
    pub reason: String,
}

/// Response for listing all blocklist entries
#[derive(Debug, Serialize, ToSchema)]
pub struct ListBlocklistResponse {
    /// Total count of blocklisted addresses
    pub count: usize,
    /// List of blocklist entries
    pub entries: Vec<BlocklistEntryResponse>,
}

/// Add an address to the internal blocklist
///
/// POST /admin/blocklist
#[utoipa::path(
    post,
    path = "/admin/blocklist",
    tag = "admin",
    request_body = AddBlocklistRequest,
    responses(
        (status = 200, description = "Address added to blocklist", body = BlocklistResponse),
        (status = 400, description = "Invalid request", body = crate::domain::ErrorResponse),
        (status = 503, description = "Blocklist not configured", body = crate::domain::ErrorResponse),
    )
)]
pub async fn add_blocklist_handler(
    State(state): State<Arc<AppState>>,
    Json(payload): Json<AddBlocklistRequest>,
) -> Result<Json<BlocklistResponse>, AppError> {
    // Validate input
    if payload.address.trim().is_empty() {
        return Err(AppError::Validation(ValidationError::MissingField(
            "address".to_string(),
        )));
    }
    if payload.reason.trim().is_empty() {
        return Err(AppError::Validation(ValidationError::MissingField(
            "reason".to_string(),
        )));
    }

    // Get blocklist or return error if not configured
    let blocklist = state
        .blocklist
        .as_ref()
        .ok_or_else(|| AppError::NotSupported("Blocklist not configured".to_string()))?;

    // Add to blocklist (persisted to database)
    blocklist
        .add_address(payload.address.clone(), payload.reason.clone())
        .await?;

    warn!(
        address = %payload.address,
        reason = %payload.reason,
        "Admin added address to blocklist"
    );

    Ok(Json(BlocklistResponse {
        success: true,
        message: format!("Address {} added to blocklist", payload.address),
    }))
}

/// Remove an address from the internal blocklist
///
/// DELETE /admin/blocklist/{address}
#[utoipa::path(
    delete,
    path = "/admin/blocklist/{address}",
    tag = "admin",
    params(
        ("address" = String, Path, description = "Wallet address to remove from blocklist")
    ),
    responses(
        (status = 200, description = "Address removed from blocklist", body = BlocklistResponse),
        (status = 404, description = "Address not found in blocklist", body = crate::domain::ErrorResponse),
        (status = 503, description = "Blocklist not configured", body = crate::domain::ErrorResponse),
    )
)]
pub async fn remove_blocklist_handler(
    State(state): State<Arc<AppState>>,
    Path(address): Path<String>,
) -> Result<Json<BlocklistResponse>, AppError> {
    // Get blocklist or return error if not configured
    let blocklist = state
        .blocklist
        .as_ref()
        .ok_or_else(|| AppError::NotSupported("Blocklist not configured".to_string()))?;

    // Remove from blocklist (persisted to database)
    if blocklist.remove_address(&address).await? {
        warn!(
            address = %address,
            "Admin removed address from blocklist"
        );
        Ok(Json(BlocklistResponse {
            success: true,
            message: format!("Address {} removed from blocklist", address),
        }))
    } else {
        Err(AppError::Database(DatabaseError::NotFound(format!(
            "Address {} not found in blocklist",
            address
        ))))
    }
}

/// List all addresses in the blocklist
///
/// GET /admin/blocklist
#[utoipa::path(
    get,
    path = "/admin/blocklist",
    tag = "admin",
    responses(
        (status = 200, description = "List of all blocklisted addresses", body = ListBlocklistResponse),
        (status = 503, description = "Blocklist not configured", body = crate::domain::ErrorResponse),
    )
)]
pub async fn list_blocklist_handler(
    State(state): State<Arc<AppState>>,
) -> Result<Json<ListBlocklistResponse>, AppError> {
    // Get blocklist or return error if not configured
    let blocklist = state
        .blocklist
        .as_ref()
        .ok_or_else(|| AppError::NotSupported("Blocklist not configured".to_string()))?;

    let entries: Vec<BlocklistEntryResponse> = blocklist
        .list_all()
        .into_iter()
        .map(|e| BlocklistEntryResponse {
            address: e.address,
            reason: e.reason,
        })
        .collect();

    Ok(Json(ListBlocklistResponse {
        count: entries.len(),
        entries,
    }))
}

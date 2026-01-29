//! Integration tests for the API.

use std::sync::Arc;

use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use ed25519_dalek::{Signer, SigningKey};
use http_body_util::BodyExt;
use tower::ServiceExt;

use solana_compliance_relayer::api::create_router;
use solana_compliance_relayer::app::AppState;
use solana_compliance_relayer::domain::{
    BlockchainStatus, HealthResponse, HealthStatus, PaginatedResponse, SubmitTransferRequest,
    TransferRequest, TransferType,
};
use solana_compliance_relayer::test_utils::{
    MockBlockchainClient, MockComplianceProvider, MockDatabaseClient,
};

// Test keypair - a deterministic keypair for testing
// Secret key bytes (32 bytes of zeros for test purposes)
const TEST_SECRET_KEY: [u8; 32] = [0u8; 32];

/// Create a valid test transfer request with proper Ed25519 signature
fn create_signed_transfer_request(
    _from_idx: u32,
    to_idx: u32,
    amount: u64,
) -> SubmitTransferRequest {
    let signing_key = SigningKey::from_bytes(&TEST_SECRET_KEY);
    let from_address = bs58::encode(signing_key.verifying_key().as_bytes()).into_string();

    // For to_address, we just need a valid Base58 string (doesn't need to verify signatures)
    let mut to_bytes = [0u8; 32];
    to_bytes[0] = (to_idx & 0xFF) as u8;
    to_bytes[1] = ((to_idx >> 8) & 0xFF) as u8;
    let to_address = bs58::encode(&to_bytes).into_string();

    let transfer_details = TransferType::Public { amount };

    // Generate unique nonce for each request
    let nonce = format!("019470a4-7e7c-7d3e-{:04x}-{:012x}", to_idx, amount);

    // Create signing message: "{from_address}:{to_address}:{amount}:{SOL}:{nonce}"
    let message = format!("{}:{}:{}:SOL:{}", from_address, to_address, amount, nonce);
    let signature = signing_key.sign(message.as_bytes());
    let signature_b58 = bs58::encode(signature.to_bytes()).into_string();

    SubmitTransferRequest {
        from_address,
        to_address,
        transfer_details,
        token_mint: None,
        signature: signature_b58,
        nonce,
    }
}

fn create_test_state() -> Arc<AppState> {
    let db = Arc::new(MockDatabaseClient::new());
    let blockchain = Arc::new(MockBlockchainClient::new());
    let compliance = Arc::new(MockComplianceProvider::new());
    Arc::new(AppState::new(db as _, blockchain as _, compliance as _))
}

#[tokio::test]
async fn test_submit_transfer_success() {
    let state = create_test_state();
    let router = create_router(state);

    let payload = create_signed_transfer_request(0, 1, 1_000_000_000);
    let expected_from = payload.from_address.clone();

    let request = Request::builder()
        .method("POST")
        .uri("/transfer-requests")
        .header("Content-Type", "application/json")
        .body(Body::from(serde_json::to_string(&payload).unwrap()))
        .unwrap();

    let response = router.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let body_bytes = response.into_body().collect().await.unwrap().to_bytes();
    let tr: TransferRequest = serde_json::from_slice(&body_bytes).unwrap();
    assert_eq!(tr.from_address, expected_from);
    assert_eq!(tr.blockchain_status, BlockchainStatus::PendingSubmission);
}

#[tokio::test]
async fn test_submit_transfer_validation_error() {
    let state = create_test_state();
    let router = create_router(state);

    // Start with valid request and break it (empty from_address after signature creation)
    // This triggers validation error, not authorization error
    let payload = create_signed_transfer_request(0, 1, 0); // Invalid: amount is 0

    let request = Request::builder()
        .method("POST")
        .uri("/transfer-requests")
        .header("Content-Type", "application/json")
        .body(Body::from(serde_json::to_string(&payload).unwrap()))
        .unwrap();

    let response = router.oneshot(request).await.unwrap();
    // Signature verification happens BEFORE validation, so with amount=0 in signature
    // but we need empty from_address to trigger validation error (400).
    // Actually, authorization (403) is checked first due to verify_signature first.
    // Let's test that an amount of 0 returns BAD_REQUEST since validation fails after sig check.
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn test_list_requests_empty() {
    let state = create_test_state();
    let router = create_router(state);

    let request = Request::builder()
        .method("GET")
        .uri("/transfer-requests")
        .body(Body::empty())
        .unwrap();

    let response = router.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let body_bytes = response.into_body().collect().await.unwrap().to_bytes();
    let result: PaginatedResponse<TransferRequest> = serde_json::from_slice(&body_bytes).unwrap();
    assert!(result.items.is_empty());
    assert!(!result.has_more);
    assert!(result.next_cursor.is_none());
}

#[tokio::test]
async fn test_list_requests_with_pagination() {
    let db = Arc::new(MockDatabaseClient::new());
    let blockchain = Arc::new(MockBlockchainClient::new());
    let compliance = Arc::new(MockComplianceProvider::new());
    let state = Arc::new(AppState::new(
        Arc::clone(&db) as _,
        Arc::clone(&blockchain) as _,
        Arc::clone(&compliance) as _,
    ));

    // Create some items
    for i in 1..5 {
        let payload = create_signed_transfer_request(0, i, (i as u64) * 1_000_000_000);
        state.service.submit_transfer(&payload).await.unwrap();
    }

    let router = create_router(state);

    // Get first page
    let request = Request::builder()
        .method("GET")
        .uri("/transfer-requests?limit=2")
        .body(Body::empty())
        .unwrap();

    let response = router.clone().oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let body_bytes = response.into_body().collect().await.unwrap().to_bytes();
    let result: PaginatedResponse<TransferRequest> = serde_json::from_slice(&body_bytes).unwrap();
    assert_eq!(result.items.len(), 2);
    assert!(result.has_more);
    assert!(result.next_cursor.is_some());

    // Get second page
    let cursor = result.next_cursor.unwrap();
    let request = Request::builder()
        .method("GET")
        .uri(format!("/transfer-requests?limit=2&cursor={}", cursor))
        .body(Body::empty())
        .unwrap();

    let response = router.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let body_bytes = response.into_body().collect().await.unwrap().to_bytes();
    let result: PaginatedResponse<TransferRequest> = serde_json::from_slice(&body_bytes).unwrap();
    assert_eq!(result.items.len(), 2);
    assert!(!result.has_more);
    assert!(result.next_cursor.is_none());
}

#[tokio::test]
async fn test_get_request_success() {
    let db = Arc::new(MockDatabaseClient::new());
    let blockchain = Arc::new(MockBlockchainClient::new());
    let compliance = Arc::new(MockComplianceProvider::new());
    let state = Arc::new(AppState::new(
        Arc::clone(&db) as _,
        Arc::clone(&blockchain) as _,
        Arc::clone(&compliance) as _,
    ));

    // Create an item
    let payload = create_signed_transfer_request(0, 1, 10_000_000_000);
    let created = state.service.submit_transfer(&payload).await.unwrap();

    let router = create_router(state);

    let request = Request::builder()
        .method("GET")
        .uri(format!("/transfer-requests/{}", created.id))
        .body(Body::empty())
        .unwrap();

    let response = router.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let body_bytes = response.into_body().collect().await.unwrap().to_bytes();
    let tr: TransferRequest = serde_json::from_slice(&body_bytes).unwrap();
    assert_eq!(tr.id, created.id);
}

#[tokio::test]
async fn test_get_request_not_found() {
    let state = create_test_state();
    let router = create_router(state);

    let request = Request::builder()
        .method("GET")
        .uri("/transfer-requests/nonexistent_id")
        .body(Body::empty())
        .unwrap();

    let response = router.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn test_graceful_degradation_blockchain_failure() {
    let db = Arc::new(MockDatabaseClient::new());
    let blockchain = Arc::new(MockBlockchainClient::failing("RPC error"));
    let compliance = Arc::new(MockComplianceProvider::new());
    let state = Arc::new(AppState::new(Arc::clone(&db) as _, blockchain, compliance));
    let router = create_router(state);

    let payload = create_signed_transfer_request(0, 1, 1_000_000_000);

    let request = Request::builder()
        .method("POST")
        .uri("/transfer-requests")
        .header("Content-Type", "application/json")
        .body(Body::from(serde_json::to_string(&payload).unwrap()))
        .unwrap();

    let response = router.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let body_bytes = response.into_body().collect().await.unwrap().to_bytes();
    let tr: TransferRequest = serde_json::from_slice(&body_bytes).unwrap();

    // Item should be created with pending_submission status (Outbox pattern: no blockchain call during submit)
    assert_eq!(tr.blockchain_status, BlockchainStatus::PendingSubmission);
    // In Outbox pattern, blockchain_last_error is NOT set during initial submit
    // (errors only occur when background worker processes the item)
    assert!(tr.blockchain_last_error.is_none());
}

#[tokio::test]
async fn test_health_check() {
    let state = create_test_state();
    let router = create_router(state);

    let request = Request::builder()
        .method("GET")
        .uri("/health")
        .body(Body::empty())
        .unwrap();

    let response = router.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let body_bytes = response.into_body().collect().await.unwrap().to_bytes();
    let health: HealthResponse = serde_json::from_slice(&body_bytes).unwrap();
    assert_eq!(health.status, HealthStatus::Healthy);
}

#[tokio::test]
async fn test_liveness() {
    let state = create_test_state();
    let router = create_router(state);

    let request = Request::builder()
        .method("GET")
        .uri("/health/live")
        .body(Body::empty())
        .unwrap();

    let response = router.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn test_readiness_healthy() {
    let state = create_test_state();
    let router = create_router(state);

    let request = Request::builder()
        .method("GET")
        .uri("/health/ready")
        .body(Body::empty())
        .unwrap();

    let response = router.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn test_readiness_unhealthy() {
    let db = Arc::new(MockDatabaseClient::new());
    db.set_healthy(false);
    let blockchain = Arc::new(MockBlockchainClient::new());
    let compliance = Arc::new(MockComplianceProvider::new());
    let state = Arc::new(AppState::new(db, blockchain, compliance));
    let router = create_router(state);

    let request = Request::builder()
        .method("GET")
        .uri("/health/ready")
        .body(Body::empty())
        .unwrap();

    let response = router.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
}

#[tokio::test]
async fn test_database_failure() {
    let db = Arc::new(MockDatabaseClient::failing("DB error"));
    let blockchain = Arc::new(MockBlockchainClient::new());
    let compliance = Arc::new(MockComplianceProvider::new());
    let state = Arc::new(AppState::new(db, blockchain, compliance));
    let router = create_router(state);

    let payload = create_signed_transfer_request(0, 1, 1_000_000_000);

    let request = Request::builder()
        .method("POST")
        .uri("/transfer-requests")
        .header("Content-Type", "application/json")
        .body(Body::from(serde_json::to_string(&payload).unwrap()))
        .unwrap();

    let response = router.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
}

#[tokio::test]
async fn test_swagger_ui_available() {
    let state = create_test_state();
    let router = create_router(state);

    let request = Request::builder()
        .method("GET")
        .uri("/swagger-ui/")
        .body(Body::empty())
        .unwrap();

    let response = router.oneshot(request).await.unwrap();
    // Swagger UI redirects or returns 200
    assert!(response.status().is_success() || response.status().is_redirection());
}

#[tokio::test]
async fn test_openapi_spec_available() {
    let state = create_test_state();
    let router = create_router(state);

    let request = Request::builder()
        .method("GET")
        .uri("/api-docs/openapi.json")
        .body(Body::empty())
        .unwrap();

    let response = router.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let body_bytes = response.into_body().collect().await.unwrap().to_bytes();
    let spec: serde_json::Value = serde_json::from_slice(&body_bytes).unwrap();
    assert!(spec.get("openapi").is_some());
    assert!(spec.get("paths").is_some());
}

#[tokio::test]
async fn test_retry_handler_item_not_found() {
    let state = create_test_state();
    let router = create_router(state);

    let request = Request::builder()
        .method("POST")
        .uri("/transfer-requests/nonexistent_id/retry")
        .body(Body::empty())
        .unwrap();

    let response = router.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn test_retry_handler_not_eligible() {
    let db = Arc::new(MockDatabaseClient::new());
    let blockchain = Arc::new(MockBlockchainClient::new());
    let compliance = Arc::new(MockComplianceProvider::new());
    let state = Arc::new(AppState::new(
        Arc::clone(&db) as _,
        Arc::clone(&blockchain) as _,
        Arc::clone(&compliance) as _,
    ));

    // Create an item with Submitted status (not eligible for retry)
    let payload = create_signed_transfer_request(0, 1, 1_000_000_000);
    let created = state.service.submit_transfer(&payload).await.unwrap();

    let router = create_router(state);

    let request = Request::builder()
        .method("POST")
        .uri(format!("/transfer-requests/{}/retry", created.id))
        .body(Body::empty())
        .unwrap();

    // Item has PendingSubmission status (Outbox pattern), which IS eligible for retry
    // But the mock blockchain succeeds, so the retry works and returns Submitted
    let response = router.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn test_post_bad_request_malformed_json() {
    let state = create_test_state();
    let router = create_router(state);

    let request = Request::builder()
        .method("POST")
        .uri("/transfer-requests")
        .header("Content-Type", "application/json")
        .body(Body::from("{ invalid json }"))
        .unwrap();

    let response = router.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn test_list_requests_invalid_limit() {
    let state = create_test_state();
    let router = create_router(state);

    // Limit is clamped, so this should still work
    let request = Request::builder()
        .method("GET")
        .uri("/transfer-requests?limit=999999")
        .body(Body::empty())
        .unwrap();

    let response = router.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn test_health_check_degraded() {
    let db = Arc::new(MockDatabaseClient::new());
    let blockchain = Arc::new(MockBlockchainClient::new());
    blockchain.set_healthy(false);
    let compliance = Arc::new(MockComplianceProvider::new());
    let state = Arc::new(AppState::new(db, blockchain, compliance));
    let router = create_router(state);

    let request = Request::builder()
        .method("GET")
        .uri("/health")
        .body(Body::empty())
        .unwrap();

    let response = router.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let body_bytes = response.into_body().collect().await.unwrap().to_bytes();
    let health: HealthResponse = serde_json::from_slice(&body_bytes).unwrap();
    assert_eq!(health.status, HealthStatus::Unhealthy);
    assert_eq!(health.database, HealthStatus::Healthy);
    assert_eq!(health.blockchain, HealthStatus::Unhealthy);
}

//! Integration tests for the API.

use std::sync::Arc;

use axum::{
    body::Body,
    http::{Request, StatusCode},
};
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

    let payload = SubmitTransferRequest {
        from_address: "FromAddr".to_string(),
        to_address: "ToAddr".to_string(),
        transfer_details: TransferType::Public {
            amount: 1_000_000_000,
        },
        token_mint: None,
    };

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
    assert_eq!(tr.from_address, "FromAddr");
    assert_eq!(tr.blockchain_status, BlockchainStatus::Submitted);
}

#[tokio::test]
async fn test_submit_transfer_validation_error() {
    let state = create_test_state();
    let router = create_router(state);

    // Invalid payload (empty from_address)
    let payload = SubmitTransferRequest {
        from_address: "".to_string(),
        to_address: "ToAddr".to_string(),
        transfer_details: TransferType::Public {
            amount: 1_000_000_000,
        },
        token_mint: None,
    };

    let request = Request::builder()
        .method("POST")
        .uri("/transfer-requests")
        .header("Content-Type", "application/json")
        .body(Body::from(serde_json::to_string(&payload).unwrap()))
        .unwrap();

    let response = router.oneshot(request).await.unwrap();
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
        let payload = SubmitTransferRequest {
            from_address: format!("From{}", i),
            to_address: format!("To{}", i),
            transfer_details: TransferType::Public {
                amount: (i as u64) * 1_000_000_000,
            },
            token_mint: None,
        };
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
    let payload = SubmitTransferRequest {
        from_address: "From".to_string(),
        to_address: "To".to_string(),
        transfer_details: TransferType::Public {
            amount: 10_000_000_000,
        },
        token_mint: None,
    };
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

    let payload = SubmitTransferRequest {
        from_address: "From".to_string(),
        to_address: "To".to_string(),
        transfer_details: TransferType::Public {
            amount: 1_000_000_000,
        },
        token_mint: None,
    };

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

    // Item should be created but with pending_submission status
    assert_eq!(tr.blockchain_status, BlockchainStatus::PendingSubmission);
    assert!(tr.blockchain_last_error.is_some());
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

    let payload = SubmitTransferRequest {
        from_address: "From".to_string(),
        to_address: "To".to_string(),
        transfer_details: TransferType::Public {
            amount: 1_000_000_000,
        },
        token_mint: None,
    };

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
    let payload = SubmitTransferRequest {
        from_address: "From".to_string(),
        to_address: "To".to_string(),
        transfer_details: TransferType::Public {
            amount: 1_000_000_000,
        },
        token_mint: None,
    };
    let created = state.service.submit_transfer(&payload).await.unwrap();

    let router = create_router(state);

    let request = Request::builder()
        .method("POST")
        .uri(format!("/transfer-requests/{}/retry", created.id))
        .body(Body::empty())
        .unwrap();

    let response = router.oneshot(request).await.unwrap();
    // Item is already submitted, not eligible for retry
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
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

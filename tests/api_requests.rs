//! Additional integration tests for specific request flows.

use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use http_body_util::BodyExt;
use std::sync::Arc;
use tower::ServiceExt;

use solana_compliance_relayer::api::create_router;
use solana_compliance_relayer::app::AppState;
use solana_compliance_relayer::domain::{
    PaginatedResponse, SubmitTransferRequest, TransferRequest,
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
async fn test_full_transfer_lifecycle_flow() {
    let state = create_test_state();
    let router = create_router(state);

    // 1. POST - Create Transfer Request
    let create_payload = SubmitTransferRequest {
        from_address: "SenderAddress".to_string(),
        to_address: "ReceiverAddress".to_string(),
        amount_sol: 50.0,
    };

    let create_request = Request::builder()
        .method("POST")
        .uri("/transfer-requests")
        .header("Content-Type", "application/json")
        .body(Body::from(serde_json::to_string(&create_payload).unwrap()))
        .unwrap();

    let create_response = router.clone().oneshot(create_request).await.unwrap();
    assert_eq!(create_response.status(), StatusCode::OK);

    let body_bytes = create_response
        .into_body()
        .collect()
        .await
        .unwrap()
        .to_bytes();
    let created_request: TransferRequest = serde_json::from_slice(&body_bytes).unwrap();
    let request_id = created_request.id;
    assert_eq!(created_request.from_address, "SenderAddress");
    assert_eq!(created_request.amount_sol, 50.0);

    // 2. GET - Retrieve the created request by ID
    let get_request = Request::builder()
        .method("GET")
        .uri(format!("/transfer-requests/{}", request_id))
        .body(Body::empty())
        .unwrap();

    let get_response = router.clone().oneshot(get_request).await.unwrap();
    assert_eq!(get_response.status(), StatusCode::OK);

    let body_bytes = get_response.into_body().collect().await.unwrap().to_bytes();
    let retrieved_request: TransferRequest = serde_json::from_slice(&body_bytes).unwrap();
    assert_eq!(retrieved_request.id, request_id);
    assert_eq!(retrieved_request.to_address, "ReceiverAddress");

    // 3. GET - List requests and verify the new request is present
    let list_request = Request::builder()
        .method("GET")
        .uri("/transfer-requests?limit=10")
        .body(Body::empty())
        .unwrap();

    let list_response = router.clone().oneshot(list_request).await.unwrap();
    assert_eq!(list_response.status(), StatusCode::OK);

    let body_bytes = list_response
        .into_body()
        .collect()
        .await
        .unwrap()
        .to_bytes();
    let list_result: PaginatedResponse<TransferRequest> =
        serde_json::from_slice(&body_bytes).unwrap();
    assert!(list_result.items.iter().any(|i| i.id == request_id));
}

#[tokio::test]
async fn test_post_bad_request_validation() {
    let state = create_test_state();
    let router = create_router(state);

    // Missing required field or invalid data (e.g., empty from_address)
    let bad_payload = SubmitTransferRequest {
        from_address: "".to_string(), // Invalid
        to_address: "ValidAddress".to_string(),
        amount_sol: 10.0,
    };

    let request = Request::builder()
        .method("POST")
        .uri("/transfer-requests")
        .header("Content-Type", "application/json")
        .body(Body::from(serde_json::to_string(&bad_payload).unwrap()))
        .unwrap();

    let response = router.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

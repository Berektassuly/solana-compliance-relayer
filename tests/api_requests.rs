//! Additional integration tests for specific request flows.

use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use ed25519_dalek::{Signer, SigningKey};
use http_body_util::BodyExt;
use std::sync::Arc;
use tower::ServiceExt;

use solana_compliance_relayer::api::create_router;
use solana_compliance_relayer::app::AppState;
use solana_compliance_relayer::domain::{
    PaginatedResponse, SubmitTransferRequest, TransferRequest, TransferType,
};
use solana_compliance_relayer::test_utils::{
    MockBlockchainClient, MockComplianceProvider, MockDatabaseClient,
};

/// Deterministic test secret key for reproducible tests
const TEST_SECRET_KEY: [u8; 32] = [
    1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 26,
    27, 28, 29, 30, 31, 32,
];

/// Helper to create a properly signed transfer request
fn create_signed_transfer_request(to_idx: u32, amount: u64) -> SubmitTransferRequest {
    let signing_key = SigningKey::from_bytes(&TEST_SECRET_KEY);
    let from_address = bs58::encode(signing_key.verifying_key().as_bytes()).into_string();

    // For to_address, we just need a valid Base58 string (doesn't need to verify signatures)
    let mut to_bytes = [0u8; 32];
    to_bytes[0] = (to_idx & 0xFF) as u8;
    to_bytes[1] = ((to_idx >> 8) & 0xFF) as u8;
    let to_address = bs58::encode(&to_bytes).into_string();

    let transfer_details = TransferType::Public { amount };

    // Create signing message: "{from_address}:{to_address}:{amount}:{SOL}"
    let message = format!("{}:{}:{}:SOL", from_address, to_address, amount);
    let signature = signing_key.sign(message.as_bytes());
    let signature_b58 = bs58::encode(signature.to_bytes()).into_string();

    SubmitTransferRequest {
        from_address,
        to_address,
        transfer_details,
        token_mint: None,
        signature: signature_b58,
    }
}

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

    // 1. POST - Create Transfer Request with valid signature
    let create_payload = create_signed_transfer_request(1, 50_000_000_000);
    let expected_from = create_payload.from_address.clone();
    let expected_to = create_payload.to_address.clone();

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
    assert_eq!(created_request.from_address, expected_from);
    assert_eq!(
        created_request.transfer_details,
        TransferType::Public {
            amount: 50_000_000_000
        }
    );

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
    assert_eq!(retrieved_request.to_address, expected_to);

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

    // Create a valid signed request but with amount=0 to trigger validation error
    // (signature verification passes, but validation fails)
    let bad_payload = create_signed_transfer_request(1, 0); // Invalid: amount is 0

    let request = Request::builder()
        .method("POST")
        .uri("/transfer-requests")
        .header("Content-Type", "application/json")
        .body(Body::from(serde_json::to_string(&bad_payload).unwrap()))
        .unwrap();

    let response = router.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

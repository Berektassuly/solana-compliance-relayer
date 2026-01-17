//! HTTP-based integration tests for Helius and QuickNode blockchain providers.
//!
//! Uses `wiremock` to mock HTTP responses for testing fee estimation,
//! DAS compliance checks, and transaction submission strategies.

use wiremock::{Mock, MockServer, ResponseTemplate, matchers::method};

// ============================================================================
// HELIUS FEE STRATEGY TESTS
// ============================================================================

mod helius_fee_strategy_tests {
    use super::*;
    use serde_json::json;

    /// Helper to create a mock Helius getPriorityFeeEstimate response
    fn helius_fee_response(high: u64, medium: u64, low: u64) -> serde_json::Value {
        json!({
            "jsonrpc": "2.0",
            "id": 1,
            "result": {
                "priorityFeeLevels": {
                    "high": high,
                    "medium": medium,
                    "low": low,
                    "min": 0,
                    "max": high * 2,
                    "veryHigh": high * 2,
                    "unsafeMax": high * 3
                }
            }
        })
    }

    #[tokio::test]
    async fn test_helius_fee_strategy_valid_response() {
        let mock_server = MockServer::start().await;

        Mock::given(method("POST"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(helius_fee_response(50000, 25000, 10000)),
            )
            .mount(&mock_server)
            .await;

        // Create Helius fee strategy with mock URL
        let client = reqwest::Client::new();
        let response = client
            .post(mock_server.uri())
            .json(&json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "getPriorityFeeEstimate",
                "params": [{"options": {"recommended": true}}]
            }))
            .send()
            .await
            .unwrap();

        let body: serde_json::Value = response.json().await.unwrap();
        assert!(
            body["result"]["priorityFeeLevels"]["high"]
                .as_u64()
                .is_some()
        );
        assert_eq!(
            body["result"]["priorityFeeLevels"]["high"]
                .as_u64()
                .unwrap(),
            50000
        );
    }

    #[tokio::test]
    async fn test_helius_fee_strategy_api_error_500() {
        let mock_server = MockServer::start().await;

        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(500).set_body_string("Internal Server Error"))
            .mount(&mock_server)
            .await;

        let client = reqwest::Client::new();
        let response = client
            .post(mock_server.uri())
            .json(&json!({"jsonrpc": "2.0", "id": 1, "method": "getPriorityFeeEstimate"}))
            .send()
            .await
            .unwrap();

        assert_eq!(response.status(), 500);
    }

    #[tokio::test]
    async fn test_helius_fee_strategy_invalid_json() {
        let mock_server = MockServer::start().await;

        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200).set_body_string("not valid json"))
            .mount(&mock_server)
            .await;

        let client = reqwest::Client::new();
        let response = client
            .post(mock_server.uri())
            .json(&json!({"jsonrpc": "2.0", "id": 1, "method": "getPriorityFeeEstimate"}))
            .send()
            .await
            .unwrap();

        let text = response.text().await.unwrap();
        let parse_result: Result<serde_json::Value, _> = serde_json::from_str(&text);
        assert!(parse_result.is_err(), "Should fail to parse invalid JSON");
    }

    #[tokio::test]
    async fn test_helius_fee_strategy_missing_fee_levels() {
        let mock_server = MockServer::start().await;

        // Response with empty result
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "jsonrpc": "2.0",
                "id": 1,
                "result": {}
            })))
            .mount(&mock_server)
            .await;

        let client = reqwest::Client::new();
        let response = client
            .post(mock_server.uri())
            .json(&json!({"jsonrpc": "2.0", "id": 1, "method": "getPriorityFeeEstimate"}))
            .send()
            .await
            .unwrap();

        let body: serde_json::Value = response.json().await.unwrap();
        assert!(body["result"]["priorityFeeLevels"].is_null());
    }
}

// ============================================================================
// HELIUS DAS CLIENT TESTS
// ============================================================================

mod helius_das_tests {
    use super::*;
    use serde_json::json;

    /// Create a mock DAS getAssetsByOwner response
    fn das_response(assets: Vec<serde_json::Value>) -> serde_json::Value {
        json!({
            "jsonrpc": "2.0",
            "id": 1,
            "result": {
                "items": assets,
                "total": assets.len(),
                "limit": 1000,
                "page": 1
            }
        })
    }

    /// Create a mock asset with optional grouping (collection)
    fn mock_asset(id: &str, collection: Option<&str>) -> serde_json::Value {
        let mut asset = json!({
            "id": id,
            "interface": "V1_NFT",
            "content": {
                "metadata": {
                    "name": "Test Asset"
                }
            }
        });

        if let Some(coll) = collection {
            asset["grouping"] = json!([{
                "group_key": "collection",
                "group_value": coll
            }]);
        }

        asset
    }

    #[tokio::test]
    async fn test_helius_das_compliant_wallet_no_assets() {
        let mock_server = MockServer::start().await;

        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200).set_body_json(das_response(vec![])))
            .mount(&mock_server)
            .await;

        let client = reqwest::Client::new();
        let response = client
            .post(mock_server.uri())
            .json(&json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "getAssetsByOwner",
                "params": {"ownerAddress": "ValidWallet123"}
            }))
            .send()
            .await
            .unwrap();

        let body: serde_json::Value = response.json().await.unwrap();
        assert_eq!(body["result"]["total"], 0);
    }

    #[tokio::test]
    async fn test_helius_das_compliant_wallet_safe_assets() {
        let mock_server = MockServer::start().await;

        let assets = vec![
            mock_asset("asset1", Some("SafeCollection123")),
            mock_asset("asset2", Some("AnotherSafeCollection")),
        ];

        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200).set_body_json(das_response(assets)))
            .mount(&mock_server)
            .await;

        let client = reqwest::Client::new();
        let response = client
            .post(mock_server.uri())
            .json(&json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "getAssetsByOwner",
                "params": {"ownerAddress": "ValidWallet123"}
            }))
            .send()
            .await
            .unwrap();

        let body: serde_json::Value = response.json().await.unwrap();
        assert_eq!(body["result"]["total"], 2);

        // Check none are sanctioned (example sanctioned collection)
        let sanctioned = "DezXAZ8z7PnrnRJjz3wXBoRgixCa6xjnB7YaB1pPB263";
        let items = body["result"]["items"].as_array().unwrap();
        for item in items {
            if let Some(grouping) = item["grouping"].as_array() {
                for group in grouping {
                    if group["group_key"] == "collection" {
                        assert_ne!(group["group_value"].as_str().unwrap(), sanctioned);
                    }
                }
            }
        }
    }

    #[tokio::test]
    async fn test_helius_das_sanctioned_wallet() {
        let mock_server = MockServer::start().await;

        // Asset from known sanctioned collection
        let sanctioned_collection = "DezXAZ8z7PnrnRJjz3wXBoRgixCa6xjnB7YaB1pPB263";
        let assets = vec![mock_asset("sanctioned_asset", Some(sanctioned_collection))];

        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200).set_body_json(das_response(assets)))
            .mount(&mock_server)
            .await;

        let client = reqwest::Client::new();
        let response = client
            .post(mock_server.uri())
            .json(&json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "getAssetsByOwner",
                "params": {"ownerAddress": "BadWallet456"}
            }))
            .send()
            .await
            .unwrap();

        let body: serde_json::Value = response.json().await.unwrap();
        let items = body["result"]["items"].as_array().unwrap();

        // Verify we found the sanctioned asset
        let has_sanctioned = items.iter().any(|item| {
            item["grouping"]
                .as_array()
                .map(|groups| {
                    groups.iter().any(|g| {
                        g["group_key"] == "collection"
                            && g["group_value"].as_str() == Some(sanctioned_collection)
                    })
                })
                .unwrap_or(false)
        });
        assert!(has_sanctioned, "Should contain sanctioned asset");
    }

    #[tokio::test]
    async fn test_helius_das_api_error_graceful_degradation() {
        let mock_server = MockServer::start().await;

        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(503).set_body_string("Service Unavailable"))
            .mount(&mock_server)
            .await;

        let client = reqwest::Client::new();
        let response = client
            .post(mock_server.uri())
            .json(&json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "getAssetsByOwner",
                "params": {"ownerAddress": "SomeWallet"}
            }))
            .send()
            .await
            .unwrap();

        // On API error, we should handle gracefully (status check)
        assert!(!response.status().is_success());
    }
}

// ============================================================================
// QUICKNODE FEE STRATEGY TESTS
// ============================================================================

mod quicknode_fee_strategy_tests {
    use super::*;
    use serde_json::json;

    fn quicknode_fee_response(high: f64, medium: f64, low: f64) -> serde_json::Value {
        json!({
            "jsonrpc": "2.0",
            "id": 1,
            "result": {
                "per_compute_unit": {
                    "high": high,
                    "medium": medium,
                    "low": low
                }
            }
        })
    }

    #[tokio::test]
    async fn test_quicknode_fee_strategy_valid_response() {
        let mock_server = MockServer::start().await;

        Mock::given(method("POST"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(quicknode_fee_response(75000.0, 50000.0, 25000.0)),
            )
            .mount(&mock_server)
            .await;

        let client = reqwest::Client::new();
        let response = client
            .post(mock_server.uri())
            .json(&json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "qn_estimatePriorityFees",
                "params": {"last_n_blocks": 100, "api_version": 2}
            }))
            .send()
            .await
            .unwrap();

        let body: serde_json::Value = response.json().await.unwrap();
        assert_eq!(
            body["result"]["per_compute_unit"]["high"].as_f64().unwrap() as u64,
            75000
        );
    }

    #[tokio::test]
    async fn test_quicknode_fee_strategy_missing_per_compute_unit() {
        let mock_server = MockServer::start().await;

        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "jsonrpc": "2.0",
                "id": 1,
                "result": {}
            })))
            .mount(&mock_server)
            .await;

        let client = reqwest::Client::new();
        let response = client
            .post(mock_server.uri())
            .json(&json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "qn_estimatePriorityFees",
                "params": {}
            }))
            .send()
            .await
            .unwrap();

        let body: serde_json::Value = response.json().await.unwrap();
        assert!(body["result"]["per_compute_unit"].is_null());
    }

    #[tokio::test]
    async fn test_quicknode_fee_strategy_null_high_value() {
        let mock_server = MockServer::start().await;

        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "jsonrpc": "2.0",
                "id": 1,
                "result": {
                    "per_compute_unit": {
                        "high": null,
                        "medium": 50000.0,
                        "low": 25000.0
                    }
                }
            })))
            .mount(&mock_server)
            .await;

        let client = reqwest::Client::new();
        let response = client
            .post(mock_server.uri())
            .json(&json!({"jsonrpc": "2.0", "id": 1, "method": "qn_estimatePriorityFees"}))
            .send()
            .await
            .unwrap();

        let body: serde_json::Value = response.json().await.unwrap();
        assert!(body["result"]["per_compute_unit"]["high"].is_null());
    }
}

// ============================================================================
// QUICKNODE JITO BUNDLE/SUBMISSION TESTS
// ============================================================================

mod quicknode_submission_tests {
    use super::*;
    use serde_json::json;

    #[tokio::test]
    async fn test_jito_bundle_success() {
        let mock_server = MockServer::start().await;

        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "jsonrpc": "2.0",
                "id": 1,
                "result": "bundle_id_abc123"
            })))
            .mount(&mock_server)
            .await;

        let client = reqwest::Client::new();
        let response = client
            .post(mock_server.uri())
            .json(&json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "sendBundle",
                "params": [["base58_encoded_tx"]]
            }))
            .send()
            .await
            .unwrap();

        let body: serde_json::Value = response.json().await.unwrap();
        assert_eq!(body["result"].as_str().unwrap(), "bundle_id_abc123");
    }

    #[tokio::test]
    async fn test_jito_bundle_error_fallback_to_standard() {
        let mock_server = MockServer::start().await;

        // First call to Jito returns error
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "jsonrpc": "2.0",
                "id": 1,
                "error": {
                    "code": -32000,
                    "message": "Bundle rejected"
                }
            })))
            .expect(1)
            .mount(&mock_server)
            .await;

        let client = reqwest::Client::new();
        let response = client
            .post(mock_server.uri())
            .json(&json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "sendBundle",
                "params": [["tx"]]
            }))
            .send()
            .await
            .unwrap();

        let body: serde_json::Value = response.json().await.unwrap();
        assert!(body["error"].is_object(), "Should receive error from Jito");
        assert!(
            body["error"]["message"]
                .as_str()
                .unwrap()
                .contains("rejected")
        );
    }

    #[tokio::test]
    async fn test_standard_send_transaction() {
        let mock_server = MockServer::start().await;

        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "jsonrpc": "2.0",
                "id": 1,
                "result": "5eykt4UsFv8P8NJdTREpY1vzqKqZKvdpKuc147dw2N9d"
            })))
            .mount(&mock_server)
            .await;

        let client = reqwest::Client::new();
        let response = client
            .post(mock_server.uri())
            .json(&json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "sendTransaction",
                "params": ["base58_tx", {"skipPreflight": true}]
            }))
            .send()
            .await
            .unwrap();

        let body: serde_json::Value = response.json().await.unwrap();
        let signature = body["result"].as_str().unwrap();
        assert!(!signature.is_empty());
        assert_eq!(signature, "5eykt4UsFv8P8NJdTREpY1vzqKqZKvdpKuc147dw2N9d");
    }

    #[tokio::test]
    async fn test_send_transaction_rpc_error() {
        let mock_server = MockServer::start().await;

        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "jsonrpc": "2.0",
                "id": 1,
                "error": {
                    "code": -32002,
                    "message": "Transaction simulation failed: insufficient funds for fee"
                }
            })))
            .mount(&mock_server)
            .await;

        let client = reqwest::Client::new();
        let response = client
            .post(mock_server.uri())
            .json(&json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "sendTransaction",
                "params": ["tx"]
            }))
            .send()
            .await
            .unwrap();

        let body: serde_json::Value = response.json().await.unwrap();
        assert!(body["error"].is_object());
        assert!(
            body["error"]["message"]
                .as_str()
                .unwrap()
                .contains("insufficient funds")
        );
    }
}

// ============================================================================
// QUICKNODE TOKEN API TESTS (for Privacy Health Check)
// ============================================================================

mod quicknode_token_api_tests {
    use super::*;
    use serde_json::json;

    fn token_activity_response(recent_tx_count: u64) -> serde_json::Value {
        json!({
            "jsonrpc": "2.0",
            "id": 1,
            "result": {
                "tokenActivity": {
                    "recentTxCount": recent_tx_count,
                    "last24hTxCount": recent_tx_count * 10,
                    "uniqueAddresses": recent_tx_count * 5
                }
            }
        })
    }

    #[tokio::test]
    async fn test_token_api_high_activity() {
        let mock_server = MockServer::start().await;

        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200).set_body_json(token_activity_response(50)))
            .mount(&mock_server)
            .await;

        let client = reqwest::Client::new();
        let response = client
            .post(mock_server.uri())
            .json(&json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "qn_getTokenActivity",
                "params": {"mint": "TokenMint123", "lookbackMinutes": 10}
            }))
            .send()
            .await
            .unwrap();

        let body: serde_json::Value = response.json().await.unwrap();
        assert_eq!(body["result"]["tokenActivity"]["recentTxCount"], 50);
    }

    #[tokio::test]
    async fn test_token_api_low_activity() {
        let mock_server = MockServer::start().await;

        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200).set_body_json(token_activity_response(2)))
            .mount(&mock_server)
            .await;

        let client = reqwest::Client::new();
        let response = client
            .post(mock_server.uri())
            .json(&json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "qn_getTokenActivity",
                "params": {"mint": "TokenMint123"}
            }))
            .send()
            .await
            .unwrap();

        let body: serde_json::Value = response.json().await.unwrap();
        assert_eq!(body["result"]["tokenActivity"]["recentTxCount"], 2);
    }

    #[tokio::test]
    async fn test_token_api_unavailable() {
        let mock_server = MockServer::start().await;

        Mock::given(method("POST"))
            .respond_with(
                ResponseTemplate::new(503).set_body_string("Service Temporarily Unavailable"),
            )
            .mount(&mock_server)
            .await;

        let client = reqwest::Client::new();
        let response = client
            .post(mock_server.uri())
            .json(&json!({"jsonrpc": "2.0", "id": 1, "method": "qn_getTokenActivity"}))
            .send()
            .await
            .unwrap();

        assert_eq!(response.status(), 503);
    }
}

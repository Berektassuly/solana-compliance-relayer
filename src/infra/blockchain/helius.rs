//! Helius RPC provider integration.
//!
//! Implements Helius-specific features:
//! - Priority Fee Estimation via `getPriorityFeeEstimate`
//! - Digital Asset Standard (DAS) API for compliance checks
//! - Smart Transaction submission (future enhancement)
//!
//! # Usage
//! The Helius features are auto-activated when the RPC URL contains `helius-rpc.com`
//! or when `HELIUS_API_KEY` environment variable is set.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tracing::{debug, info, warn};

use crate::domain::{AppError, BlockchainError};

use super::strategies::FeeStrategy;

// ============================================================================
// SANCTIONED COLLECTIONS (Mock for Hackathon)
// ============================================================================

/// Sanctioned collection addresses
///
/// In production, this would be fetched from a compliance oracle or database.
/// For the hackathon demo, we use a static list of mock addresses.
pub const SANCTIONED_COLLECTIONS: &[&str] = &[
    // Mock sanctioned collection addresses for demo
    "SANCTIONED111111111111111111111111111111111",
    "SANCTIONED222222222222222222222222222222222",
    // Add known malicious NFT collections here
    "DezXAZ8z7PnrnRJjz3wXBoRgixCa6xjnB7YaB1pPB263", // Example: Known scam collection
];

// ============================================================================
// HELIUS FEE STRATEGY
// ============================================================================

/// Helius Priority Fee Response
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HeliusPriorityFeeResponse {
    pub priority_fee_estimate: Option<f64>,
    #[allow(dead_code)]
    pub priority_fee_levels: Option<HeliusPriorityFeeLevels>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HeliusPriorityFeeLevels {
    #[allow(dead_code)]
    pub min: Option<f64>,
    #[allow(dead_code)]
    pub low: Option<f64>,
    pub medium: Option<f64>,
    pub high: Option<f64>,
    #[allow(dead_code)]
    pub very_high: Option<f64>,
    #[allow(dead_code)]
    pub unsafe_max: Option<f64>,
}

/// Helius-specific fee strategy using `getPriorityFeeEstimate`
///
/// This strategy provides more accurate fee estimates by analyzing the
/// accounts involved in the transaction.
pub struct HeliusFeeStrategy {
    rpc_url: String,
    http_client: reqwest::Client,
    default_fee: u64,
}

impl HeliusFeeStrategy {
    pub fn new(rpc_url: &str) -> Self {
        info!("Helius Priority Fee Strategy activated!");
        Self {
            rpc_url: rpc_url.to_string(),
            http_client: reqwest::Client::new(),
            default_fee: 100,
        }
    }
}

#[derive(Debug, Serialize)]
struct JsonRpcRequest<T: Serialize> {
    jsonrpc: &'static str,
    id: &'static str,
    method: String,
    params: T,
}

#[derive(Debug, Deserialize)]
struct JsonRpcResponse<T> {
    result: Option<T>,
    error: Option<JsonRpcError>,
}

#[derive(Debug, Deserialize)]
struct JsonRpcError {
    #[allow(dead_code)]
    code: i64,
    message: String,
}

#[async_trait]
impl FeeStrategy for HeliusFeeStrategy {
    async fn get_priority_fee(&self, serialized_tx: Option<&str>) -> u64 {
        // Build params based on whether we have a transaction
        let params = if let Some(tx) = serialized_tx {
            serde_json::json!({
                "transaction": tx,
                "options": {
                    "includeAllPriorityFeeLevels": true
                }
            })
        } else {
            // Without a transaction, request global estimate
            serde_json::json!({
                "options": {
                    "includeAllPriorityFeeLevels": true
                }
            })
        };

        let request = JsonRpcRequest {
            jsonrpc: "2.0",
            id: "helius-fee",
            method: "getPriorityFeeEstimate".to_string(),
            params: vec![params],
        };

        match self
            .http_client
            .post(&self.rpc_url)
            .json(&request)
            .send()
            .await
        {
            Ok(response) => {
                match response
                    .json::<JsonRpcResponse<HeliusPriorityFeeResponse>>()
                    .await
                {
                    Ok(rpc_response) => {
                        if let Some(error) = rpc_response.error {
                            debug!(error = %error.message, "Helius fee API error, using default");
                            return self.default_fee;
                        }

                        if let Some(result) = rpc_response.result {
                            // Prefer the direct estimate, fall back to levels
                            let fee = result
                                .priority_fee_estimate
                                .or_else(|| {
                                    result.priority_fee_levels.as_ref().and_then(|l| l.high)
                                })
                                .or_else(|| {
                                    result.priority_fee_levels.as_ref().and_then(|l| l.medium)
                                })
                                .unwrap_or(self.default_fee as f64);

                            let fee_u64 = fee as u64;
                            info!(
                                priority_fee = %fee_u64,
                                "Helius priority fee applied (micro-lamports)"
                            );
                            return fee_u64;
                        }
                        debug!("Helius response missing fee data, using default");
                        self.default_fee
                    }
                    Err(e) => {
                        debug!(error = %e, "Failed to parse Helius response, using default");
                        self.default_fee
                    }
                }
            }
            Err(e) => {
                debug!(error = %e, "Helius API request failed, using default");
                self.default_fee
            }
        }
    }

    fn name(&self) -> &'static str {
        "Helius (getPriorityFeeEstimate)"
    }
}

// ============================================================================
// HELIUS DAS CLIENT
// ============================================================================

/// Helius Digital Asset Standard (DAS) client for compliance checks
///
/// Uses the `getAssetsByOwner` API to fetch all assets owned by a wallet
/// and checks them against a sanctioned collection list.
pub struct HeliusDasClient {
    rpc_url: String,
    http_client: reqwest::Client,
}

impl HeliusDasClient {
    pub fn new(rpc_url: &str) -> Self {
        info!("Helius DAS Check enabled");
        Self {
            rpc_url: rpc_url.to_string(),
            http_client: reqwest::Client::new(),
        }
    }

    /// Check if a wallet holds any assets from sanctioned collections
    ///
    /// # Arguments
    /// * `owner` - The wallet address (Base58) to check
    ///
    /// # Returns
    /// * `Ok(true)` - Wallet is compliant (no sanctioned assets)
    /// * `Ok(false)` - Wallet holds sanctioned assets
    /// * `Err(_)` - API error (caller should decide how to handle)
    pub async fn check_wallet_compliance(&self, owner: &str) -> Result<bool, AppError> {
        info!(wallet = %owner, "Helius DAS Check: Scanning wallet assets");

        let params = serde_json::json!({
            "ownerAddress": owner,
            "page": 1,
            "limit": 100,
            "displayOptions": {
                "showCollectionMetadata": true
            }
        });

        let request = JsonRpcRequest {
            jsonrpc: "2.0",
            id: "helius-das",
            method: "getAssetsByOwner".to_string(),
            params,
        };

        let response = self
            .http_client
            .post(&self.rpc_url)
            .json(&request)
            .send()
            .await
            .map_err(|e| {
                AppError::Blockchain(BlockchainError::HeliusApiError(format!(
                    "DAS API request failed: {}",
                    e
                )))
            })?;

        let rpc_response: JsonRpcResponse<DasAssetsResponse> =
            response.json().await.map_err(|e| {
                AppError::Blockchain(BlockchainError::HeliusApiError(format!(
                    "Failed to parse DAS response: {}",
                    e
                )))
            })?;

        if let Some(error) = rpc_response.error {
            return Err(AppError::Blockchain(BlockchainError::HeliusApiError(
                error.message,
            )));
        }

        let result = rpc_response.result.ok_or_else(|| {
            AppError::Blockchain(BlockchainError::HeliusApiError(
                "Empty DAS response".to_string(),
            ))
        })?;

        // Check each asset against sanctioned collections
        for asset in &result.items {
            if let Some(grouping) = &asset.grouping {
                for group in grouping {
                    if group.group_key == "collection" {
                        if let Some(group_value) = &group.group_value {
                            if SANCTIONED_COLLECTIONS.contains(&group_value.as_str()) {
                                warn!(
                                    wallet = %owner,
                                    sanctioned_collection = %group_value,
                                    asset_id = %asset.id,
                                    "DAS Check FAILED: Wallet holds sanctioned asset"
                                );
                                return Ok(false);
                            }
                        }
                    }
                }
            }
        }

        let asset_count = result.items.len();
        info!(
            wallet = %owner,
            assets_checked = asset_count,
            "DAS Check PASSED: No sanctioned assets found"
        );
        Ok(true)
    }
}

// ============================================================================
// DAS RESPONSE TYPES
// ============================================================================

#[derive(Debug, Deserialize)]
pub struct DasAssetsResponse {
    #[allow(dead_code)]
    pub total: Option<u64>,
    #[allow(dead_code)]
    pub limit: Option<u64>,
    #[allow(dead_code)]
    pub page: Option<u64>,
    pub items: Vec<DasAsset>,
}

#[derive(Debug, Deserialize)]
pub struct DasAsset {
    pub id: String,
    #[allow(dead_code)]
    pub interface: Option<String>,
    #[allow(dead_code)]
    pub content: Option<DasAssetContent>,
    pub grouping: Option<Vec<DasAssetGroup>>,
}

#[derive(Debug, Deserialize)]
pub struct DasAssetContent {
    #[allow(dead_code)]
    pub json_uri: Option<String>,
    #[allow(dead_code)]
    pub metadata: Option<DasAssetMetadata>,
}

#[derive(Debug, Deserialize)]
pub struct DasAssetMetadata {
    #[allow(dead_code)]
    pub name: Option<String>,
    #[allow(dead_code)]
    pub symbol: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct DasAssetGroup {
    pub group_key: String,
    pub group_value: Option<String>,
}

// ============================================================================
// TESTS
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sanctioned_collection_check() {
        assert!(SANCTIONED_COLLECTIONS.contains(&"SANCTIONED111111111111111111111111111111111"));
        assert!(!SANCTIONED_COLLECTIONS.contains(&"SAFE11111111111111111111111111111111111111"));
    }

    #[tokio::test]
    async fn test_helius_fee_strategy_name() {
        // We can't test the actual API call without mocking, but we can test the name
        let strategy = HeliusFeeStrategy {
            rpc_url: "https://test.helius-rpc.com".to_string(),
            http_client: reqwest::Client::new(),
            default_fee: 100,
        };
        assert_eq!(strategy.name(), "Helius (getPriorityFeeEstimate)");
    }

    #[test]
    fn test_das_client_creation() {
        // Just verify the client can be created
        let _client = HeliusDasClient::new("https://test.helius-rpc.com");
    }
}

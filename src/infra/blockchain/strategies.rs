//! Provider Strategy Pattern for blockchain RPC providers.
//!
//! This module implements the Strategy Pattern to abstract blockchain provider
//! differences (Helius, QuickNode, Standard RPC) behind unified traits.
//! The system auto-detects the provider type and activates premium features accordingly.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tracing::{debug, info};

use crate::domain::AppError;

// ============================================================================
// PROVIDER TYPE DETECTION
// ============================================================================

/// Enum representing detected RPC provider type
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RpcProviderType {
    /// Helius RPC - Premium features available (Priority Fees, DAS)
    Helius,
    /// QuickNode RPC - Priority fee estimation available
    QuickNode,
    /// Standard Solana RPC - Basic functionality only
    Standard,
}

impl RpcProviderType {
    /// Detect provider type from RPC URL
    ///
    /// # Examples
    /// ```
    /// use solana_compliance_relayer::infra::blockchain::strategies::RpcProviderType;
    ///
    /// assert_eq!(
    ///     RpcProviderType::detect("https://mainnet.helius-rpc.com/?api-key=xxx"),
    ///     RpcProviderType::Helius
    /// );
    /// ```
    pub fn detect(rpc_url: &str) -> Self {
        let url_lower = rpc_url.to_lowercase();

        if url_lower.contains("helius-rpc.com") || url_lower.contains("helius.xyz") {
            RpcProviderType::Helius
        } else if url_lower.contains("quiknode.pro") || url_lower.contains("quicknode.com") {
            RpcProviderType::QuickNode
        } else {
            RpcProviderType::Standard
        }
    }

    /// Check if this provider supports Helius DAS API
    pub fn supports_das(&self) -> bool {
        matches!(self, RpcProviderType::Helius)
    }

    /// Get a human-readable name for logging
    pub fn name(&self) -> &'static str {
        match self {
            RpcProviderType::Helius => "Helius",
            RpcProviderType::QuickNode => "QuickNode",
            RpcProviderType::Standard => "Standard RPC",
        }
    }
}

// ============================================================================
// FEE STRATEGY TRAIT
// ============================================================================

/// Strategy for estimating priority fees
///
/// Different RPC providers have different APIs for fee estimation:
/// - Helius: `getPriorityFeeEstimate` with transaction-aware estimation
/// - QuickNode: `qn_estimatePriorityFees` with global estimation
/// - Standard: Static fallback value
#[async_trait]
pub trait FeeStrategy: Send + Sync {
    /// Get recommended priority fee in micro-lamports
    ///
    /// # Arguments
    /// * `serialized_tx` - Optional Base58-encoded serialized transaction
    ///                     (used by Helius for per-account fee estimation)
    async fn get_priority_fee(&self, serialized_tx: Option<&str>) -> u64;

    /// Human-readable strategy name for logging
    fn name(&self) -> &'static str;
}

// ============================================================================
// SUBMISSION STRATEGY TRAIT
// ============================================================================

/// Strategy for submitting transactions
///
/// Different providers offer different submission methods:
/// - Standard: `sendTransaction` RPC
/// - Helius: Smart Transactions with optimistic confirmation
/// - QuickNode: Ghost Mode via Jito bundles (private submission)
#[async_trait]
pub trait SubmissionStrategy: Send + Sync {
    /// Submit a serialized transaction
    ///
    /// # Arguments
    /// * `serialized_tx` - Base58-encoded serialized transaction
    /// * `skip_preflight` - Whether to skip preflight simulation
    ///
    /// # Returns
    /// Transaction signature on success
    async fn submit_transaction(
        &self,
        serialized_tx: &str,
        skip_preflight: bool,
    ) -> Result<String, AppError>;

    /// Human-readable strategy name for logging
    fn name(&self) -> &'static str;

    /// Returns true if this strategy supports private/MEV-protected submission
    ///
    /// Private submission bypasses the public mempool for enhanced privacy.
    /// Currently only available with QuickNode's Jito integration.
    fn supports_private_submission(&self) -> bool {
        false // Default: standard submission
    }
}

// ============================================================================
// FALLBACK FEE STRATEGY
// ============================================================================

/// Fallback fee strategy that returns a static default value
///
/// Used when no provider-specific fee estimation is available
pub struct FallbackFeeStrategy {
    default_fee: u64,
}

impl FallbackFeeStrategy {
    /// Create a new fallback strategy with the default fee (100 micro-lamports)
    pub fn new() -> Self {
        Self { default_fee: 100 }
    }

    /// Create a new fallback strategy with a custom default fee
    pub fn with_fee(fee: u64) -> Self {
        Self { default_fee: fee }
    }
}

impl Default for FallbackFeeStrategy {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl FeeStrategy for FallbackFeeStrategy {
    async fn get_priority_fee(&self, _serialized_tx: Option<&str>) -> u64 {
        debug!(
            fee = self.default_fee,
            "Using fallback priority fee (micro-lamports)"
        );
        self.default_fee
    }

    fn name(&self) -> &'static str {
        "Fallback (static)"
    }
}

// ============================================================================
// QUICKNODE FEE STRATEGY
// ============================================================================

/// Response structure for QuickNode's qn_estimatePriorityFees API
#[derive(Debug, Deserialize)]
pub struct QuickNodePriorityFeeResponse {
    pub per_compute_unit: Option<QuickNodePriorityFeeLevel>,
}

/// Priority fee levels from QuickNode API (values in micro-lamports)
#[derive(Debug, Deserialize)]
pub struct QuickNodePriorityFeeLevel {
    pub high: Option<f64>,
    #[allow(dead_code)]
    pub medium: Option<f64>,
    #[allow(dead_code)]
    pub low: Option<f64>,
}

/// QuickNode-specific fee strategy using `qn_estimatePriorityFees`
pub struct QuickNodeFeeStrategy {
    rpc_url: String,
    http_client: reqwest::Client,
    default_fee: u64,
}

impl QuickNodeFeeStrategy {
    pub fn new(rpc_url: &str) -> Self {
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
    id: u64,
    method: String,
    params: T,
}

#[derive(Debug, Deserialize)]
struct JsonRpcResponse<T> {
    result: Option<T>,
    #[allow(dead_code)]
    error: Option<serde_json::Value>,
}

#[async_trait]
impl FeeStrategy for QuickNodeFeeStrategy {
    async fn get_priority_fee(&self, _serialized_tx: Option<&str>) -> u64 {
        let params = serde_json::json!({
            "last_n_blocks": 100,
            "api_version": 2
        });

        let request = JsonRpcRequest {
            jsonrpc: "2.0",
            id: 1,
            method: "qn_estimatePriorityFees".to_string(),
            params,
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
                    .json::<JsonRpcResponse<QuickNodePriorityFeeResponse>>()
                    .await
                {
                    Ok(rpc_response) => {
                        if let Some(result) = rpc_response.result {
                            if let Some(fees) = result.per_compute_unit {
                                if let Some(high) = fees.high {
                                    let fee = high as u64;
                                    info!(
                                        priority_fee = %fee,
                                        "⚡ QuickNode priority fee applied (micro-lamports)"
                                    );
                                    return fee;
                                }
                            }
                        }
                        debug!("QuickNode response missing fee data, using default");
                        self.default_fee
                    }
                    Err(e) => {
                        debug!(error = %e, "Failed to parse QuickNode response, using default");
                        self.default_fee
                    }
                }
            }
            Err(e) => {
                debug!(error = %e, "QuickNode API request failed, using default");
                self.default_fee
            }
        }
    }

    fn name(&self) -> &'static str {
        "QuickNode (qn_estimatePriorityFees)"
    }
}

// ============================================================================
// TESTS
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_provider_detection_helius() {
        assert_eq!(
            RpcProviderType::detect("https://mainnet.helius-rpc.com/?api-key=xxx"),
            RpcProviderType::Helius
        );
        assert_eq!(
            RpcProviderType::detect("https://devnet.helius-rpc.com"),
            RpcProviderType::Helius
        );
        assert_eq!(
            RpcProviderType::detect("https://rpc.helius.xyz/?api-key=test"),
            RpcProviderType::Helius
        );
    }

    #[test]
    fn test_provider_detection_quicknode() {
        assert_eq!(
            RpcProviderType::detect("https://xxx.solana-mainnet.quiknode.pro/yyy"),
            RpcProviderType::QuickNode
        );
        assert_eq!(
            RpcProviderType::detect("https://my-endpoint.quicknode.com"),
            RpcProviderType::QuickNode
        );
    }

    #[test]
    fn test_provider_detection_standard() {
        assert_eq!(
            RpcProviderType::detect("https://api.mainnet-beta.solana.com"),
            RpcProviderType::Standard
        );
        assert_eq!(
            RpcProviderType::detect("https://api.devnet.solana.com"),
            RpcProviderType::Standard
        );
        assert_eq!(
            RpcProviderType::detect("http://localhost:8899"),
            RpcProviderType::Standard
        );
    }

    #[test]
    fn test_provider_supports_das() {
        assert!(RpcProviderType::Helius.supports_das());
        assert!(!RpcProviderType::QuickNode.supports_das());
        assert!(!RpcProviderType::Standard.supports_das());
    }

    #[tokio::test]
    async fn test_fallback_fee_strategy() {
        let strategy = FallbackFeeStrategy::new();
        assert_eq!(strategy.get_priority_fee(None).await, 100);
        assert_eq!(strategy.name(), "Fallback (static)");
    }

    #[tokio::test]
    async fn test_fallback_fee_strategy_custom() {
        let strategy = FallbackFeeStrategy::with_fee(500);
        assert_eq!(strategy.get_priority_fee(None).await, 500);
    }
}

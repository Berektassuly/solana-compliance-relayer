//! QuickNode RPC provider integration.
//!
//! Implements QuickNode-specific features:
//! - Private Transaction Submission via Jito Bundles (Ghost Mode)
//! - Token API for anonymity set analysis
//! - Priority Fee Estimation (moved from strategies.rs)
//!
//! # Ghost Mode
//! Transactions are submitted directly to Jito block builders, bypassing
//! the public mempool for enhanced privacy (MEV protection).
//!
//! # Usage
//! QuickNode features are auto-activated when the RPC URL contains `quiknode.pro`
//! or `quicknode.com`.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tracing::{debug, info, warn};

use crate::domain::{AppError, BlockchainError};

use super::strategies::SubmissionStrategy;

// ============================================================================
// QUICKNODE SUBMISSION CONFIG
// ============================================================================

/// Configuration for QuickNode private submission
#[derive(Debug, Clone)]
pub struct QuickNodeSubmissionConfig {
    /// QuickNode RPC URL
    pub rpc_url: String,
    /// Enable Jito bundle submission for private transactions
    pub enable_jito_bundles: bool,
    /// Tip amount for Jito block builders (in lamports)
    pub tip_lamports: u64,
    /// Maximum retries for bundle submission
    pub max_bundle_retries: u32,
}

impl Default for QuickNodeSubmissionConfig {
    fn default() -> Self {
        Self {
            rpc_url: String::new(),
            enable_jito_bundles: true,
            tip_lamports: 1_000, // 0.000001 SOL
            max_bundle_retries: 2,
        }
    }
}

// ============================================================================
// JITO BUNDLE TYPES
// ============================================================================

/// Jito bundle submission request
#[derive(Debug, Serialize)]
struct JitoBundleRequest {
    jsonrpc: &'static str,
    id: u64,
    method: String,
    params: Vec<Vec<String>>,
}

/// Jito bundle submission response
#[derive(Debug, Deserialize)]
struct JitoBundleResponse {
    result: Option<String>,
    error: Option<JitoError>,
}

#[derive(Debug, Deserialize)]
struct JitoError {
    code: i64,
    message: String,
}

/// Standard sendTransaction request
#[derive(Debug, Serialize)]
struct SendTransactionRequest {
    jsonrpc: &'static str,
    id: u64,
    method: String,
    params: Vec<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
struct SendTransactionResponse {
    result: Option<String>,
    error: Option<RpcError>,
}

#[derive(Debug, Deserialize)]
struct RpcError {
    code: i64,
    message: String,
}

// ============================================================================
// QUICKNODE PRIVATE SUBMISSION STRATEGY
// ============================================================================

/// QuickNode Private Submission Strategy using Jito Bundles
///
/// Submits transactions directly to Jito block builders, bypassing the
/// public mempool for enhanced privacy (Ghost Mode).
///
/// # Graceful Degradation
/// If Jito submission fails, automatically falls back to standard
/// `sendTransaction` to ensure high availability.
pub struct QuickNodePrivateSubmissionStrategy {
    config: QuickNodeSubmissionConfig,
    http_client: reqwest::Client,
}

impl QuickNodePrivateSubmissionStrategy {
    /// Create a new private submission strategy
    pub fn new(config: QuickNodeSubmissionConfig) -> Self {
        info!(
            rpc_url = %config.rpc_url,
            jito_enabled = config.enable_jito_bundles,
            tip_lamports = config.tip_lamports,
            "🔒 QuickNode Private Submission Strategy (Ghost Mode) initialized"
        );
        Self {
            config,
            http_client: reqwest::Client::new(),
        }
    }

    /// Submit transaction as a Jito bundle for private submission
    async fn submit_jito_bundle(&self, serialized_tx: &str) -> Result<String, AppError> {
        debug!(
            tx_len = serialized_tx.len(),
            "Attempting Jito bundle submission"
        );

        // QuickNode's Jito integration uses qn_broadcastBundle
        let request = JitoBundleRequest {
            jsonrpc: "2.0",
            id: 1,
            method: "qn_broadcastBundle".to_string(),
            params: vec![vec![serialized_tx.to_string()]],
        };

        let response = self
            .http_client
            .post(&self.config.rpc_url)
            .json(&request)
            .send()
            .await
            .map_err(|e| {
                AppError::Blockchain(BlockchainError::JitoBundleFailed(format!(
                    "HTTP request failed: {}",
                    e
                )))
            })?;

        let bundle_response: JitoBundleResponse = response.json().await.map_err(|e| {
            AppError::Blockchain(BlockchainError::JitoBundleFailed(format!(
                "Failed to parse response: {}",
                e
            )))
        })?;

        if let Some(error) = bundle_response.error {
            // Check for "method not found" which means Jito isn't available
            if error.code == -32601 {
                return Err(AppError::Blockchain(
                    BlockchainError::PrivateSubmissionFallback(
                        "qn_broadcastBundle not available on this endpoint".to_string(),
                    ),
                ));
            }
            return Err(AppError::Blockchain(BlockchainError::JitoBundleFailed(
                error.message,
            )));
        }

        bundle_response.result.ok_or_else(|| {
            AppError::Blockchain(BlockchainError::JitoBundleFailed(
                "Empty response from Jito".to_string(),
            ))
        })
    }

    /// Submit transaction via standard sendTransaction RPC
    async fn submit_standard(
        &self,
        serialized_tx: &str,
        skip_preflight: bool,
    ) -> Result<String, AppError> {
        debug!(
            skip_preflight = skip_preflight,
            "Using standard sendTransaction"
        );

        let params = vec![
            serde_json::Value::String(serialized_tx.to_string()),
            serde_json::json!({
                "skipPreflight": skip_preflight,
                "preflightCommitment": "confirmed",
                "encoding": "base58"
            }),
        ];

        let request = SendTransactionRequest {
            jsonrpc: "2.0",
            id: 1,
            method: "sendTransaction".to_string(),
            params,
        };

        let response = self
            .http_client
            .post(&self.config.rpc_url)
            .json(&request)
            .send()
            .await
            .map_err(|e| {
                AppError::Blockchain(BlockchainError::RpcError(format!(
                    "sendTransaction HTTP failed: {}",
                    e
                )))
            })?;

        let tx_response: SendTransactionResponse = response.json().await.map_err(|e| {
            AppError::Blockchain(BlockchainError::RpcError(format!(
                "Failed to parse sendTransaction response: {}",
                e
            )))
        })?;

        if let Some(error) = tx_response.error {
            return Err(AppError::Blockchain(BlockchainError::TransactionFailed(
                format!("{}: {}", error.code, error.message),
            )));
        }

        tx_response.result.ok_or_else(|| {
            AppError::Blockchain(BlockchainError::RpcError(
                "Empty sendTransaction response".to_string(),
            ))
        })
    }
}

#[async_trait]
impl SubmissionStrategy for QuickNodePrivateSubmissionStrategy {
    async fn submit_transaction(
        &self,
        serialized_tx: &str,
        skip_preflight: bool,
    ) -> Result<String, AppError> {
        // Attempt Jito bundle submission first if enabled
        if self.config.enable_jito_bundles {
            match self.submit_jito_bundle(serialized_tx).await {
                Ok(signature) => {
                    info!(
                        signature = %signature,
                        "🔒 Ghost Mode: Transaction submitted privately via Jito bundle"
                    );
                    return Ok(signature);
                }
                Err(e) => {
                    // Check if it's a fallback-worthy error
                    let should_fallback = matches!(
                        &e,
                        AppError::Blockchain(BlockchainError::PrivateSubmissionFallback(_))
                            | AppError::Blockchain(BlockchainError::JitoBundleFailed(_))
                    );

                    if should_fallback {
                        warn!(
                            error = %e,
                            "Ghost Mode unavailable, falling back to standard submission"
                        );
                    } else {
                        // For other errors, don't fallback (e.g., network errors)
                        return Err(e);
                    }
                }
            }
        }

        // Fallback to standard sendTransaction
        let signature = self.submit_standard(serialized_tx, skip_preflight).await?;
        info!(
            signature = %signature,
            "Transaction submitted via standard sendTransaction (fallback)"
        );
        Ok(signature)
    }

    fn name(&self) -> &'static str {
        "QuickNode (Ghost Mode / Jito)"
    }

    fn supports_private_submission(&self) -> bool {
        self.config.enable_jito_bundles
    }
}

// ============================================================================
// STANDARD SUBMISSION STRATEGY
// ============================================================================

/// Standard submission strategy using sendTransaction RPC
///
/// Used as the default for non-QuickNode providers and as a fallback
/// when private submission is unavailable.
pub struct StandardSubmissionStrategy {
    rpc_url: String,
    http_client: reqwest::Client,
}

impl StandardSubmissionStrategy {
    /// Create a new standard submission strategy
    pub fn new(rpc_url: &str) -> Self {
        debug!(rpc_url = %rpc_url, "Standard submission strategy initialized");
        Self {
            rpc_url: rpc_url.to_string(),
            http_client: reqwest::Client::new(),
        }
    }
}

#[async_trait]
impl SubmissionStrategy for StandardSubmissionStrategy {
    async fn submit_transaction(
        &self,
        serialized_tx: &str,
        skip_preflight: bool,
    ) -> Result<String, AppError> {
        let params = vec![
            serde_json::Value::String(serialized_tx.to_string()),
            serde_json::json!({
                "skipPreflight": skip_preflight,
                "preflightCommitment": "confirmed",
                "encoding": "base58"
            }),
        ];

        let request = SendTransactionRequest {
            jsonrpc: "2.0",
            id: 1,
            method: "sendTransaction".to_string(),
            params,
        };

        let response = self
            .http_client
            .post(&self.rpc_url)
            .json(&request)
            .send()
            .await
            .map_err(|e| {
                AppError::Blockchain(BlockchainError::RpcError(format!(
                    "sendTransaction failed: {}",
                    e
                )))
            })?;

        let tx_response: SendTransactionResponse = response.json().await.map_err(|e| {
            AppError::Blockchain(BlockchainError::RpcError(format!(
                "Failed to parse response: {}",
                e
            )))
        })?;

        if let Some(error) = tx_response.error {
            return Err(AppError::Blockchain(BlockchainError::TransactionFailed(
                format!("{}: {}", error.code, error.message),
            )));
        }

        let signature = tx_response.result.ok_or_else(|| {
            AppError::Blockchain(BlockchainError::RpcError("Empty response".to_string()))
        })?;

        debug!(signature = %signature, "Transaction submitted via sendTransaction");
        Ok(signature)
    }

    fn name(&self) -> &'static str {
        "Standard (sendTransaction)"
    }

    fn supports_private_submission(&self) -> bool {
        false
    }
}

// ============================================================================
// QUICKNODE TOKEN API CLIENT
// ============================================================================

/// Token activity information for anonymity set analysis
#[derive(Debug, Clone)]
pub struct TokenActivityInfo {
    /// Token mint address
    pub token_mint: String,
    /// Number of recent transactions
    pub recent_tx_count: u64,
    /// Timestamp of last activity (if available)
    pub last_activity_timestamp: Option<DateTime<Utc>>,
    /// Whether this is an estimate (cached/approximated)
    pub is_estimate: bool,
}

/// QuickNode Token API client for privacy health checks
///
/// Fetches token metadata and recent transaction history to assess
/// the anonymity set health before confidential transfers.
pub struct QuickNodeTokenApiClient {
    rpc_url: String,
    http_client: reqwest::Client,
}

impl QuickNodeTokenApiClient {
    /// Create a new Token API client
    pub fn new(rpc_url: &str) -> Self {
        info!(rpc_url = %rpc_url, "QuickNode Token API client initialized");
        Self {
            rpc_url: rpc_url.to_string(),
            http_client: reqwest::Client::new(),
        }
    }

    /// Get recent transaction activity for a token mint
    ///
    /// Uses QuickNode's enhanced RPC methods to fetch token activity.
    /// Falls back to signature counting if advanced APIs aren't available.
    pub async fn get_recent_activity(
        &self,
        token_mint: &str,
        lookback_minutes: u64,
    ) -> Result<TokenActivityInfo, AppError> {
        debug!(
            token_mint = %token_mint,
            lookback_minutes = lookback_minutes,
            "Fetching token activity for privacy health check"
        );

        // Try QuickNode's qn_getTokenMetadata first
        match self.get_token_metadata(token_mint).await {
            Ok(info) => {
                debug!(
                    token_mint = %token_mint,
                    recent_tx_count = info.recent_tx_count,
                    "Token metadata fetched successfully"
                );
                Ok(info)
            }
            Err(e) => {
                // Fallback: Use getSignaturesForAddress to count recent transactions
                debug!(
                    error = %e,
                    "qn_getTokenMetadata failed, falling back to signature counting"
                );
                self.count_recent_signatures(token_mint, lookback_minutes)
                    .await
            }
        }
    }

    /// Fetch token metadata using QuickNode's enhanced API
    async fn get_token_metadata(&self, token_mint: &str) -> Result<TokenActivityInfo, AppError> {
        #[derive(Debug, Serialize)]
        struct TokenMetadataRequest {
            jsonrpc: &'static str,
            id: u64,
            method: String,
            params: serde_json::Value,
        }

        #[derive(Debug, Deserialize)]
        struct TokenMetadataResponse {
            result: Option<TokenMetadata>,
            error: Option<RpcError>,
        }

        #[derive(Debug, Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct TokenMetadata {
            #[serde(default)]
            holder_count: Option<u64>,
            #[serde(default)]
            transfer_count_24h: Option<u64>,
        }

        let request = TokenMetadataRequest {
            jsonrpc: "2.0",
            id: 1,
            method: "qn_getTokenMetadata".to_string(),
            params: serde_json::json!({ "mint": token_mint }),
        };

        let response = self
            .http_client
            .post(&self.rpc_url)
            .json(&request)
            .send()
            .await
            .map_err(|e| {
                AppError::Blockchain(BlockchainError::QuickNodeApiError(format!(
                    "Token API request failed: {}",
                    e
                )))
            })?;

        let metadata_response: TokenMetadataResponse = response.json().await.map_err(|e| {
            AppError::Blockchain(BlockchainError::QuickNodeApiError(format!(
                "Failed to parse token metadata: {}",
                e
            )))
        })?;

        if let Some(error) = metadata_response.error {
            return Err(AppError::Blockchain(BlockchainError::QuickNodeApiError(
                error.message,
            )));
        }

        let metadata = metadata_response.result.ok_or_else(|| {
            AppError::Blockchain(BlockchainError::QuickNodeApiError(
                "Empty token metadata response".to_string(),
            ))
        })?;

        // Use transfer_count_24h as a proxy for recent activity
        // Scale it down for our lookback window (e.g., 10 minutes = 10/1440 of 24h)
        let daily_transfers = metadata.transfer_count_24h.unwrap_or(0);
        let lookback_fraction = 10.0 / 1440.0; // 10 minutes / 24 hours
        let estimated_recent = (daily_transfers as f64 * lookback_fraction).round() as u64;

        Ok(TokenActivityInfo {
            token_mint: token_mint.to_string(),
            recent_tx_count: estimated_recent.max(1), // At least 1 if token exists
            last_activity_timestamp: None,
            is_estimate: true,
        })
    }

    /// Fallback: Count recent signatures for the token mint address
    async fn count_recent_signatures(
        &self,
        token_mint: &str,
        _lookback_minutes: u64,
    ) -> Result<TokenActivityInfo, AppError> {
        #[derive(Debug, Serialize)]
        struct SignaturesRequest {
            jsonrpc: &'static str,
            id: u64,
            method: String,
            params: Vec<serde_json::Value>,
        }

        #[derive(Debug, Deserialize)]
        struct SignaturesResponse {
            result: Option<Vec<SignatureInfo>>,
            error: Option<RpcError>,
        }

        #[derive(Debug, Deserialize)]
        struct SignatureInfo {
            #[allow(dead_code)]
            signature: String,
        }

        let request = SignaturesRequest {
            jsonrpc: "2.0",
            id: 1,
            method: "getSignaturesForAddress".to_string(),
            params: vec![
                serde_json::Value::String(token_mint.to_string()),
                serde_json::json!({ "limit": 100 }),
            ],
        };

        let response = self
            .http_client
            .post(&self.rpc_url)
            .json(&request)
            .send()
            .await
            .map_err(|e| {
                AppError::Blockchain(BlockchainError::QuickNodeApiError(format!(
                    "Signature fetch failed: {}",
                    e
                )))
            })?;

        let sig_response: SignaturesResponse = response.json().await.map_err(|e| {
            AppError::Blockchain(BlockchainError::QuickNodeApiError(format!(
                "Failed to parse signatures: {}",
                e
            )))
        })?;

        if let Some(error) = sig_response.error {
            return Err(AppError::Blockchain(BlockchainError::QuickNodeApiError(
                error.message,
            )));
        }

        let signatures = sig_response.result.unwrap_or_default();

        Ok(TokenActivityInfo {
            token_mint: token_mint.to_string(),
            recent_tx_count: signatures.len() as u64,
            last_activity_timestamp: None,
            is_estimate: false,
        })
    }
}

// ============================================================================
// TESTS
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_quicknode_submission_config_default() {
        let config = QuickNodeSubmissionConfig::default();
        assert!(config.enable_jito_bundles);
        assert_eq!(config.tip_lamports, 1_000);
        assert_eq!(config.max_bundle_retries, 2);
    }

    #[test]
    fn test_standard_submission_strategy_name() {
        let strategy = StandardSubmissionStrategy::new("https://api.devnet.solana.com");
        assert_eq!(strategy.name(), "Standard (sendTransaction)");
        assert!(!strategy.supports_private_submission());
    }

    #[test]
    fn test_quicknode_private_submission_strategy_name() {
        let config = QuickNodeSubmissionConfig {
            rpc_url: "https://test.quiknode.pro/xxx".to_string(),
            enable_jito_bundles: true,
            tip_lamports: 1_000,
            max_bundle_retries: 2,
        };
        let strategy = QuickNodePrivateSubmissionStrategy::new(config);
        assert_eq!(strategy.name(), "QuickNode (Ghost Mode / Jito)");
        assert!(strategy.supports_private_submission());
    }

    #[test]
    fn test_token_activity_info() {
        let info = TokenActivityInfo {
            token_mint: "test_mint".to_string(),
            recent_tx_count: 10,
            last_activity_timestamp: Some(Utc::now()),
            is_estimate: false,
        };
        assert_eq!(info.token_mint, "test_mint");
        assert_eq!(info.recent_tx_count, 10);
        assert!(!info.is_estimate);
    }

    #[test]
    fn test_quicknode_token_api_client_creation() {
        let client = QuickNodeTokenApiClient::new("https://test.quiknode.pro/xxx");
        assert_eq!(client.rpc_url, "https://test.quiknode.pro/xxx");
    }
}

# Implementation Plan: QuickNode Privacy Features

This document outlines the implementation plan for two new privacy-enhancing features that leverage QuickNode's RPC services: **Ghost Mode** (Private Transaction Submission) and **Privacy Health Check** (Anonymity Set Analysis).

---

## Executive Summary

The proposed implementation introduces two new features to enhance privacy and reliability:

1. **Ghost Mode**: Private transaction submission via QuickNode's Jito Bundle integration, bypassing the public mempool
2. **Privacy Health Check**: Algorithmic assessment of network anonymity set before confidential transfer submission

Both features follow the existing Strategy Pattern architecture and include graceful degradation to ensure high availability.

---

## Architecture Overview

The existing codebase follows a clean layered architecture with the Strategy Pattern for provider-specific features:

```
src/
├── api/          # HTTP handlers, routing
├── app/          # Business logic, workers
│   ├── service.rs       # AppService orchestration
│   └── worker.rs        # BlockchainRetryWorker
├── domain/       # Traits, types, errors
│   ├── traits.rs        # BlockchainClient, etc.
│   └── error.rs         # Error types
└── infra/        # External integrations
    └── blockchain/
        ├── strategies.rs    # FeeStrategy, SubmissionStrategy traits
        ├── helius.rs        # Helius-specific implementations
        └── solana.rs        # RpcBlockchainClient
```

---

## Proposed Changes

### Summary Table

| File | Action | Description |
|------|--------|-------------|
| `src/infra/blockchain/quicknode.rs` | **NEW** | QuickNode-specific strategies and Token API client |
| `src/infra/privacy/mod.rs` | **NEW** | Privacy analytics module |
| `src/infra/privacy/health_check.rs` | **NEW** | Anonymity set health check service |
| `src/infra/blockchain/strategies.rs` | MODIFY | Add `submit_private` method to `SubmissionStrategy` |
| `src/infra/blockchain/solana.rs` | MODIFY | Integrate QuickNode submission strategy |
| `src/infra/blockchain/mod.rs` | MODIFY | Re-export QuickNode types |
| `src/infra/mod.rs` | MODIFY | Add privacy module |
| `src/app/worker.rs` | MODIFY | Add privacy health check before submission |
| `src/app/service.rs` | MODIFY | Inject privacy service dependency |
| `src/app/state.rs` | MODIFY | Add privacy service to AppState |
| `src/domain/error.rs` | MODIFY | Add QuickNode and Privacy error variants |
| `src/main.rs` | MODIFY | Initialize privacy health check service |
| `Cargo.toml` | MODIFY | Add `jito-sdk` dependency (optional) |

---

## Feature 1: Ghost Mode - Private Transaction Submission

### Core Objective
Mitigate network-level observability risks (front-running, timing analysis) by submitting transactions privately via QuickNode's Jito Bundle integration.

### New Files

---

#### [NEW] `src/infra/blockchain/quicknode.rs`

QuickNode-specific strategy implementations for private transaction submission.

```rust
// Module structure:
//
// ============================================================================
// QUICKNODE PRIVATE SUBMISSION STRATEGY
// ============================================================================

/// Configuration for QuickNode private submission
pub struct QuickNodeSubmissionConfig {
    pub rpc_url: String,
    pub enable_jito_bundles: bool,
    pub tip_lamports: u64,            // Tip for Jito block builder
    pub max_bundle_retries: u32,
}

/// QuickNode Private Submission Strategy using Jito Bundles
///
/// Submits transactions directly to Jito block builders, bypassing the
/// public mempool for enhanced privacy.
pub struct QuickNodePrivateSubmissionStrategy {
    config: QuickNodeSubmissionConfig,
    http_client: reqwest::Client,
    fallback_rpc_url: String,
}

impl QuickNodePrivateSubmissionStrategy {
    pub fn new(config: QuickNodeSubmissionConfig) -> Self;
    
    /// Attempt private submission via Jito bundle
    /// Falls back to standard sendTransaction on failure
    pub async fn submit_with_fallback(
        &self,
        serialized_tx: &str,
        skip_preflight: bool,
    ) -> Result<String, AppError>;
    
    /// Submit transaction as a Jito bundle
    async fn submit_jito_bundle(&self, serialized_tx: &str) -> Result<String, AppError>;
    
    /// Check if Jito endpoint is available
    async fn check_jito_availability(&self) -> bool;
}

#[async_trait]
impl SubmissionStrategy for QuickNodePrivateSubmissionStrategy {
    async fn submit_transaction(
        &self,
        serialized_tx: &str,
        skip_preflight: bool,
    ) -> Result<String, AppError>;
    
    fn name(&self) -> &'static str { "QuickNode (Jito Private)" }
    
    /// Returns true if this strategy supports private submission
    fn supports_private_submission(&self) -> bool { true }
}

// ============================================================================
// JITO BUNDLE TYPES
// ============================================================================

/// Jito bundle submission request
#[derive(Debug, Serialize)]
pub struct JitoBundleRequest {
    pub jsonrpc: &'static str,
    pub id: u64,
    pub method: String,
    pub params: JitoBundleParams,
}

#[derive(Debug, Serialize)]
pub struct JitoBundleParams {
    pub bundle: Vec<String>,  // List of base58-encoded transactions
}

/// Jito bundle submission response
#[derive(Debug, Deserialize)]
pub struct JitoBundleResponse {
    pub result: Option<String>,  // Bundle ID
    pub error: Option<JitoError>,
}

#[derive(Debug, Deserialize)]
pub struct JitoError {
    pub code: i64,
    pub message: String,
}

// ============================================================================
// QUICKNODE TOKEN API CLIENT
// ============================================================================

/// QuickNode Token API client for fetching token metadata and history
/// Used by the Privacy Health Check feature
pub struct QuickNodeTokenApiClient {
    rpc_url: String,
    http_client: reqwest::Client,
}

impl QuickNodeTokenApiClient {
    pub fn new(rpc_url: &str) -> Self;
    
    /// Get recent transaction count for a token mint
    /// Uses qn_getTokenMetadata or similar endpoint
    pub async fn get_recent_activity(
        &self,
        token_mint: &str,
        lookback_minutes: u64,
    ) -> Result<TokenActivityInfo, AppError>;
}

#[derive(Debug, Clone)]
pub struct TokenActivityInfo {
    pub token_mint: String,
    pub recent_tx_count: u64,
    pub last_activity_timestamp: Option<DateTime<Utc>>,
    pub is_estimate: bool,  // True if data is estimated/cached
}
```

**Key Design Decisions:**
- Implements `SubmissionStrategy` trait for seamless integration
- Graceful fallback to standard `sendTransaction` on any failure
- Configurable tip amount for Jito bundles
- Logs warnings on fallback for monitoring

---

### Modified Files

---

#### [MODIFY] `src/infra/blockchain/strategies.rs`

Add optional private submission capability to the `SubmissionStrategy` trait.

```diff
 /// Strategy for submitting transactions
 #[async_trait]
 pub trait SubmissionStrategy: Send + Sync {
     /// Submit a serialized transaction
     async fn submit_transaction(
         &self,
         serialized_tx: &str,
         skip_preflight: bool,
     ) -> Result<String, AppError>;
 
     /// Human-readable strategy name for logging
     fn name(&self) -> &'static str;
+
+    /// Returns true if this strategy supports private/MEV-protected submission
+    fn supports_private_submission(&self) -> bool {
+        false  // Default: standard submission
+    }
 }

+/// Standard submission strategy using sendTransaction RPC
+/// Used as fallback and for non-QuickNode providers
+pub struct StandardSubmissionStrategy {
+    rpc_url: String,
+    http_client: reqwest::Client,
+}
+
+impl StandardSubmissionStrategy {
+    pub fn new(rpc_url: &str) -> Self;
+}
+
+#[async_trait]
+impl SubmissionStrategy for StandardSubmissionStrategy {
+    async fn submit_transaction(
+        &self,
+        serialized_tx: &str,
+        skip_preflight: bool,
+    ) -> Result<String, AppError>;
+
+    fn name(&self) -> &'static str { "Standard (sendTransaction)" }
+}
```

---

#### [MODIFY] `src/infra/blockchain/solana.rs`

Integrate the submission strategy into `RpcBlockchainClient`.

```diff
 pub struct RpcBlockchainClient {
     provider: Box<dyn SolanaRpcProvider>,
     config: RpcClientConfig,
     sdk_client: Option<SolanaRpcClient>,
     keypair: Option<Keypair>,
     provider_type: super::strategies::RpcProviderType,
     fee_strategy: Box<dyn super::strategies::FeeStrategy>,
+    submission_strategy: Box<dyn super::strategies::SubmissionStrategy>,
     das_client: Option<super::helius::HeliusDasClient>,
     rpc_url: String,
 }

 impl RpcBlockchainClient {
     pub fn new(
         rpc_url: &str,
         signing_key: SigningKey,
         config: RpcClientConfig,
     ) -> Result<Self, AppError> {
         // ... existing detection logic ...
         
+        // Select submission strategy based on provider type
+        let submission_strategy: Box<dyn super::strategies::SubmissionStrategy> = 
+            match &provider_type {
+                RpcProviderType::QuickNode => {
+                    info!("🔒 QuickNode Private Submission (Jito) activated!");
+                    Box::new(super::quicknode::QuickNodePrivateSubmissionStrategy::new(
+                        super::quicknode::QuickNodeSubmissionConfig {
+                            rpc_url: rpc_url.to_string(),
+                            enable_jito_bundles: true,
+                            tip_lamports: 1_000, // 0.000001 SOL
+                            max_bundle_retries: 2,
+                        },
+                    ))
+                }
+                _ => {
+                    Box::new(super::strategies::StandardSubmissionStrategy::new(rpc_url))
+                }
+            };
+
         // ... rest of initialization ...
     }
+
+    /// Check if private submission is available
+    pub fn supports_private_submission(&self) -> bool {
+        self.submission_strategy.supports_private_submission()
+    }
 }
```

---

#### [MODIFY] `src/infra/blockchain/mod.rs`

Re-export QuickNode types.

```diff
 pub mod helius;
+pub mod quicknode;
 pub mod solana;
 pub mod strategies;

 // Re-export main types
 pub use solana::{RpcBlockchainClient, RpcClientConfig, signing_key_from_base58};

 // Re-export strategy types
-pub use strategies::{FeeStrategy, RpcProviderType};
+pub use strategies::{FeeStrategy, RpcProviderType, SubmissionStrategy, StandardSubmissionStrategy};
+
+// Re-export QuickNode-specific types
+pub use quicknode::{QuickNodePrivateSubmissionStrategy, QuickNodeTokenApiClient};

 // Re-export Helius-specific types
 pub use helius::{HeliusDasClient, HeliusFeeStrategy, SANCTIONED_COLLECTIONS};
```

---

#### [MODIFY] `src/domain/error.rs`

Add QuickNode-specific error variants to `BlockchainError`.

```diff
 #[derive(Error, Debug, Clone)]
 pub enum BlockchainError {
     // ... existing variants ...
     
     #[error("Helius API error: {0}")]
     HeliusApiError(String),
     #[error("DAS compliance check failed: {0}")]
     DasComplianceFailed(String),
+    #[error("QuickNode API error: {0}")]
+    QuickNodeApiError(String),
+    #[error("Jito bundle submission failed: {0}")]
+    JitoBundleFailed(String),
+    #[error("Private submission unavailable, falling back: {0}")]
+    PrivateSubmissionFallback(String),
 }
```

---

## Feature 2: Privacy Health Check - Anonymity Set Analysis

### Core Objective
Protect users from timing attacks by assessing network activity for a token before submission. If activity is low, delay submission to blend with future transactions.

### New Files

---

#### [NEW] `src/infra/privacy/mod.rs`

Privacy module declaration.

```rust
//! Privacy analytics and protection features.
//!
//! This module provides privacy-enhancing services for confidential transfers:
//! - Anonymity set health checks
//! - Smart delay mechanisms for timing attack mitigation

pub mod health_check;

pub use health_check::{
    PrivacyHealthCheckService, 
    PrivacyHealthCheckConfig,
    AnonymitySetHealth,
};
```

---

#### [NEW] `src/infra/privacy/health_check.rs`

Anonymity set health check service implementation.

```rust
//! Anonymity Set Health Check Service
//!
//! Assesses the recent transaction volume ("anonymity set health") for a
//! given confidential token mint before submission. Implements smart delays
//! when network activity is low to mitigate timing attacks.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use rand::Rng;
use std::sync::Arc;
use tracing::{debug, info, warn};

use crate::domain::AppError;
use crate::infra::blockchain::QuickNodeTokenApiClient;

/// Configuration for the Privacy Health Check service
#[derive(Debug, Clone)]
pub struct PrivacyHealthCheckConfig {
    /// Minimum number of recent transactions to consider "healthy"
    pub min_tx_threshold: u64,
    /// Lookback window in minutes for activity assessment
    pub lookback_minutes: u64,
    /// Maximum delay in seconds when activity is low
    pub max_delay_secs: u64,
    /// Minimum delay in seconds when activity is low
    pub min_delay_secs: u64,
    /// Whether the health check is enabled
    pub enabled: bool,
}

impl Default for PrivacyHealthCheckConfig {
    fn default() -> Self {
        Self {
            min_tx_threshold: 5,       // Require 5+ transactions
            lookback_minutes: 10,       // In the last 10 minutes
            max_delay_secs: 120,        // Max 2 minute delay
            min_delay_secs: 10,         // Min 10 second delay
            enabled: true,
        }
    }
}

/// Result of an anonymity set health check
#[derive(Debug, Clone)]
pub struct AnonymitySetHealth {
    pub token_mint: String,
    pub recent_tx_count: u64,
    pub is_healthy: bool,
    pub recommended_delay_secs: Option<u64>,
    pub checked_at: DateTime<Utc>,
}

impl AnonymitySetHealth {
    /// Create a healthy result (no delay needed)
    pub fn healthy(token_mint: String, recent_tx_count: u64) -> Self {
        Self {
            token_mint,
            recent_tx_count,
            is_healthy: true,
            recommended_delay_secs: None,
            checked_at: Utc::now(),
        }
    }

    /// Create an unhealthy result with recommended delay
    pub fn unhealthy(token_mint: String, recent_tx_count: u64, delay_secs: u64) -> Self {
        Self {
            token_mint,
            recent_tx_count,
            is_healthy: false,
            recommended_delay_secs: Some(delay_secs),
            checked_at: Utc::now(),
        }
    }
    
    /// Skip the check result (e.g., API unavailable)
    pub fn skipped(token_mint: String) -> Self {
        Self {
            token_mint,
            recent_tx_count: 0,
            is_healthy: true,  // Assume healthy to prioritize liveness
            recommended_delay_secs: None,
            checked_at: Utc::now(),
        }
    }
}

/// Privacy Health Check Service
///
/// Checks the anonymity set health for confidential token transfers
/// and recommends delays when network activity is low.
pub struct PrivacyHealthCheckService {
    config: PrivacyHealthCheckConfig,
    token_api_client: Option<Arc<QuickNodeTokenApiClient>>,
}

impl PrivacyHealthCheckService {
    /// Create a new service with QuickNode Token API client
    pub fn new(
        config: PrivacyHealthCheckConfig,
        token_api_client: Option<Arc<QuickNodeTokenApiClient>>,
    ) -> Self {
        if token_api_client.is_some() {
            info!(
                threshold = config.min_tx_threshold,
                lookback_minutes = config.lookback_minutes,
                "Privacy Health Check service initialized with QuickNode Token API"
            );
        } else {
            warn!("Privacy Health Check service initialized WITHOUT Token API (will skip checks)");
        }
        
        Self {
            config,
            token_api_client,
        }
    }

    /// Create a disabled/passthrough service
    pub fn disabled() -> Self {
        Self {
            config: PrivacyHealthCheckConfig {
                enabled: false,
                ..Default::default()
            },
            token_api_client: None,
        }
    }

    /// Check if the service is enabled and has API access
    pub fn is_operational(&self) -> bool {
        self.config.enabled && self.token_api_client.is_some()
    }

    /// Check the anonymity set health for a token mint
    ///
    /// # Arguments
    /// * `token_mint` - The token mint address to check
    ///
    /// # Returns
    /// * `AnonymitySetHealth` - Health status with optional delay recommendation
    ///
    /// # Graceful Degradation
    /// If the Token API is unavailable or returns an error, the check is skipped
    /// and the transaction proceeds immediately (prioritizing liveness).
    pub async fn check_health(&self, token_mint: &str) -> AnonymitySetHealth {
        // Skip if disabled
        if !self.config.enabled {
            debug!(token_mint = %token_mint, "Privacy health check disabled, skipping");
            return AnonymitySetHealth::skipped(token_mint.to_string());
        }

        // Skip if no API client
        let client = match &self.token_api_client {
            Some(c) => c,
            None => {
                debug!(token_mint = %token_mint, "No Token API client, skipping health check");
                return AnonymitySetHealth::skipped(token_mint.to_string());
            }
        };

        // Query recent activity
        match client.get_recent_activity(token_mint, self.config.lookback_minutes).await {
            Ok(activity) => {
                let recent_tx_count = activity.recent_tx_count;
                
                if recent_tx_count >= self.config.min_tx_threshold {
                    info!(
                        token_mint = %token_mint,
                        recent_tx_count = recent_tx_count,
                        threshold = self.config.min_tx_threshold,
                        "✅ Anonymity set HEALTHY - proceeding with submission"
                    );
                    AnonymitySetHealth::healthy(token_mint.to_string(), recent_tx_count)
                } else {
                    // Calculate randomized delay
                    let delay = self.calculate_delay(recent_tx_count);
                    
                    warn!(
                        token_mint = %token_mint,
                        recent_tx_count = recent_tx_count,
                        threshold = self.config.min_tx_threshold,
                        delay_secs = delay,
                        "⚠️ Anonymity set UNHEALTHY - recommending delay"
                    );
                    AnonymitySetHealth::unhealthy(token_mint.to_string(), recent_tx_count, delay)
                }
            }
            Err(e) => {
                // Graceful degradation: log warning and proceed
                warn!(
                    token_mint = %token_mint,
                    error = %e,
                    "Privacy health check failed - skipping to preserve liveness"
                );
                AnonymitySetHealth::skipped(token_mint.to_string())
            }
        }
    }

    /// Calculate a randomized delay based on activity level
    fn calculate_delay(&self, recent_tx_count: u64) -> u64 {
        let mut rng = rand::thread_rng();
        
        // Lower activity = longer delay (inverse relationship)
        let activity_factor = if self.config.min_tx_threshold > 0 {
            1.0 - (recent_tx_count as f64 / self.config.min_tx_threshold as f64)
        } else {
            1.0
        };
        
        let base_delay = self.config.min_delay_secs as f64 +
            (activity_factor * (self.config.max_delay_secs - self.config.min_delay_secs) as f64);
        
        // Add randomization (±30%)
        let jitter = rng.gen_range(0.7..1.3);
        let delay = (base_delay * jitter) as u64;
        
        delay.clamp(self.config.min_delay_secs, self.config.max_delay_secs)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_config_default() {
        let config = PrivacyHealthCheckConfig::default();
        assert_eq!(config.min_tx_threshold, 5);
        assert_eq!(config.lookback_minutes, 10);
        assert!(config.enabled);
    }

    #[test]
    fn test_health_result_healthy() {
        let health = AnonymitySetHealth::healthy("token123".to_string(), 10);
        assert!(health.is_healthy);
        assert!(health.recommended_delay_secs.is_none());
    }

    #[test]
    fn test_health_result_unhealthy() {
        let health = AnonymitySetHealth::unhealthy("token123".to_string(), 2, 60);
        assert!(!health.is_healthy);
        assert_eq!(health.recommended_delay_secs, Some(60));
    }

    #[test]
    fn test_health_result_skipped() {
        let health = AnonymitySetHealth::skipped("token123".to_string());
        assert!(health.is_healthy);  // Skipped = assume healthy for liveness
    }

    #[test]
    fn test_disabled_service() {
        let service = PrivacyHealthCheckService::disabled();
        assert!(!service.is_operational());
    }

    #[tokio::test]
    async fn test_check_health_disabled() {
        let service = PrivacyHealthCheckService::disabled();
        let health = service.check_health("some_mint").await;
        assert!(health.is_healthy);
    }
}
```

**Key Design Decisions:**
- Implements graceful degradation: API failures result in immediate processing
- Randomized delays prevent predictable timing patterns
- Configurable thresholds for different token activity levels
- Clean separation from blockchain submission logic

---

### Modified Files

---

#### [MODIFY] `src/infra/mod.rs`

Add privacy module.

```diff
 //! Infrastructure layer implementations.

 pub mod blockchain;
 pub mod compliance;
 pub mod database;
+pub mod privacy;

 pub use blockchain::{RpcBlockchainClient, RpcClientConfig, signing_key_from_base58};
 pub use compliance::RangeComplianceProvider;
 pub use database::{PostgresClient, PostgresConfig};
+pub use privacy::{PrivacyHealthCheckService, PrivacyHealthCheckConfig, AnonymitySetHealth};
```

---

#### [MODIFY] `src/app/worker.rs`

Integrate privacy health check before confidential transfer submission.

```diff
 use std::sync::Arc;
 use std::time::Duration;
 use tokio::sync::watch;
-use tracing::{error, info};
+use tracing::{debug, error, info, warn};
 
 use super::service::AppService;
+use crate::domain::types::TransferType;
+use crate::infra::privacy::PrivacyHealthCheckService;

 /// Configuration for the background worker
 #[derive(Debug, Clone)]
 pub struct WorkerConfig {
     /// Interval between processing batches
     pub poll_interval: Duration,
     /// Number of items to process per batch
     pub batch_size: i64,
     /// Whether the worker is enabled
     pub enabled: bool,
+    /// Whether to apply privacy health checks for confidential transfers
+    pub enable_privacy_checks: bool,
 }
 
 impl Default for WorkerConfig {
     fn default() -> Self {
         Self {
             poll_interval: Duration::from_secs(10),
             batch_size: 10,
             enabled: true,
+            enable_privacy_checks: true,
         }
     }
 }
 
 /// Background worker for processing pending blockchain submissions
 pub struct BlockchainRetryWorker {
     service: Arc<AppService>,
     config: WorkerConfig,
     shutdown_rx: watch::Receiver<bool>,
+    privacy_service: Option<Arc<PrivacyHealthCheckService>>,
 }
 
 impl BlockchainRetryWorker {
     /// Create a new worker instance
     pub fn new(
         service: Arc<AppService>,
         config: WorkerConfig,
         shutdown_rx: watch::Receiver<bool>,
+        privacy_service: Option<Arc<PrivacyHealthCheckService>>,
     ) -> Self {
         Self {
             service,
             config,
             shutdown_rx,
+            privacy_service,
         }
     }
 
+    /// Apply privacy health check for confidential transfers
+    /// Returns the delay in seconds to wait before submission, or 0 for immediate
+    async fn check_privacy_health(&self, request: &crate::domain::TransferRequest) -> u64 {
+        // Only check confidential transfers
+        let is_confidential = matches!(
+            request.transfer_details,
+            TransferType::Confidential { .. }
+        );
+        
+        if !is_confidential || !self.config.enable_privacy_checks {
+            return 0;
+        }
+        
+        let privacy_service = match &self.privacy_service {
+            Some(s) => s,
+            None => return 0,
+        };
+        
+        let token_mint = match &request.token_mint {
+            Some(mint) => mint,
+            None => return 0,
+        };
+        
+        let health = privacy_service.check_health(token_mint).await;
+        
+        if health.is_healthy {
+            0
+        } else {
+            health.recommended_delay_secs.unwrap_or(0)
+        }
+    }
 }
```

**Note:** The actual delay logic should be integrated into `process_single_submission` in `AppService` or handled by the worker to reschedule with a delay. The implementation should use `next_retry_at` with the recommended delay.

---

#### [MODIFY] `src/app/state.rs`

Add privacy service to AppState.

```diff
 use std::sync::Arc;
 
 use crate::domain::{BlockchainClient, ComplianceProvider, DatabaseClient};
+use crate::infra::privacy::PrivacyHealthCheckService;
 
 use super::service::AppService;
 
 /// Shared application state
 pub struct AppState {
     pub service: Arc<AppService>,
     pub helius_webhook_secret: Option<String>,
+    pub privacy_service: Option<Arc<PrivacyHealthCheckService>>,
 }
 
 impl AppState {
     pub fn new(
         db_client: Arc<dyn DatabaseClient>,
         blockchain_client: Arc<dyn BlockchainClient>,
         compliance_provider: Arc<dyn ComplianceProvider>,
     ) -> Self {
         Self {
             service: Arc::new(AppService::new(db_client, blockchain_client, compliance_provider)),
             helius_webhook_secret: None,
+            privacy_service: None,
         }
     }
 
     pub fn with_helius_secret(
         db_client: Arc<dyn DatabaseClient>,
         blockchain_client: Arc<dyn BlockchainClient>,
         compliance_provider: Arc<dyn ComplianceProvider>,
         helius_webhook_secret: Option<String>,
     ) -> Self {
         Self {
             service: Arc::new(AppService::new(db_client, blockchain_client, compliance_provider)),
             helius_webhook_secret,
+            privacy_service: None,
+        }
+    }
+
+    pub fn with_privacy_service(
+        mut self,
+        privacy_service: Arc<PrivacyHealthCheckService>,
+    ) -> Self {
+        self.privacy_service = Some(privacy_service);
+        self
     }
 }
```

---

#### [MODIFY] `src/main.rs`

Initialize privacy health check service.

```diff
 use solana_compliance_relayer::app::{AppState, WorkerConfig, spawn_worker};
 use solana_compliance_relayer::infra::RpcBlockchainClient;
-use solana_compliance_relayer::infra::{PostgresClient, PostgresConfig, signing_key_from_base58};
+use solana_compliance_relayer::infra::{
+    PostgresClient, PostgresConfig, signing_key_from_base58,
+    PrivacyHealthCheckService, PrivacyHealthCheckConfig,
+};
+use solana_compliance_relayer::infra::blockchain::QuickNodeTokenApiClient;

 /// Application configuration
 struct Config {
     // ... existing fields ...
     
+    /// Enable privacy health checks for confidential transfers
+    enable_privacy_checks: bool,
 }

 impl Config {
     fn from_env() -> Result<Self> {
         // ... existing parsing ...
         
+        let enable_privacy_checks = env::var("ENABLE_PRIVACY_CHECKS")
+            .map(|v| v == "true" || v == "1")
+            .unwrap_or(true);  // Enabled by default
         
         Ok(Self {
             // ... existing fields ...
+            enable_privacy_checks,
         })
     }
 }

 #[tokio::main]
 async fn main() -> Result<()> {
     // ... existing initialization ...
     
+    // Initialize privacy health check service (QuickNode only)
+    let privacy_service = if config.enable_privacy_checks {
+        use solana_compliance_relayer::infra::blockchain::RpcProviderType;
+        
+        let provider_type = RpcProviderType::detect(&config.blockchain_rpc_url);
+        
+        if matches!(provider_type, RpcProviderType::QuickNode) {
+            let token_api_client = Arc::new(QuickNodeTokenApiClient::new(&config.blockchain_rpc_url));
+            let privacy_config = PrivacyHealthCheckConfig::default();
+            let service = Arc::new(PrivacyHealthCheckService::new(privacy_config, Some(token_api_client)));
+            info!("   ✓ Privacy Health Check service initialized (QuickNode)");
+            Some(service)
+        } else {
+            info!("   ○ Privacy Health Check disabled (requires QuickNode RPC)");
+            None
+        }
+    } else {
+        info!("   ○ Privacy Health Check disabled via config");
+        None
+    };
     
     // Create application state
-    let app_state = Arc::new(AppState::with_helius_secret(
+    let mut app_state = AppState::with_helius_secret(
         Arc::new(postgres_client),
         Arc::new(blockchain_client),
         Arc::new(compliance_provider),
         config.helius_webhook_secret.clone(),
-    ));
+    );
+    
+    if let Some(privacy_svc) = privacy_service.clone() {
+        app_state = app_state.with_privacy_service(privacy_svc);
+    }
+    
+    let app_state = Arc::new(app_state);
     
     // ... rest of main ...
 }
```

---

## Dependencies

#### [MODIFY] `Cargo.toml`

No new external crates are strictly required. The implementation uses existing dependencies:
- `reqwest` for HTTP requests to Jito/QuickNode APIs
- `serde` / `serde_json` for JSON serialization
- `async-trait` for async trait implementations
- `tracing` for logging
- `rand` for delay randomization (already present)
- `chrono` for timestamps (already present)

**Optional Enhancement:**
If deep Jito integration is desired, consider adding:

```diff
 # Optional: Direct Jito SDK integration
+jito-sdk = { version = "0.2", optional = true }
+
+[features]
+default = []
+test-utils = []
+real-blockchain = []
+jito-bundles = ["jito-sdk"]
```

---

## Environment Variables

New optional environment variables:

| Variable | Default | Description |
|----------|---------|-------------|
| `ENABLE_PRIVACY_CHECKS` | `true` | Enable/disable privacy health checks |
| `PRIVACY_MIN_TX_THRESHOLD` | `5` | Minimum transactions for healthy anonymity set |
| `PRIVACY_LOOKBACK_MINUTES` | `10` | Lookback window for activity check |
| `PRIVACY_MAX_DELAY_SECS` | `120` | Maximum delay when activity is low |
| `JITO_TIP_LAMPORTS` | `1000` | Tip amount for Jito bundle builders |

---

## Verification Plan

### Automated Tests

1. **Unit Tests for QuickNode Strategies**
   ```bash
   cargo test quicknode:: --lib
   ```

2. **Unit Tests for Privacy Health Check**
   ```bash
   cargo test privacy:: --lib
   ```

3. **Integration Tests**
   ```bash
   cargo test --test integration_test
   ```

### Manual Verification

1. **Ghost Mode Verification**
   - Configure QuickNode RPC URL
   - Submit a transaction
   - Verify logs show "QuickNode Private Submission (Jito) activated"
   - Check that fallback works when Jito is unavailable

2. **Privacy Health Check Verification**
   - Configure QuickNode RPC URL
   - Submit a confidential transfer
   - Verify logs show anonymity set health check
   - Test with low-activity token to verify delay recommendation

3. **Graceful Degradation Testing**
   - Test with non-QuickNode RPC (should fall back to standard)
   - Test with API errors (should proceed immediately)
   - Test with disabled features (should bypass checks)

---

## Risk Mitigation

| Risk | Mitigation |
|------|------------|
| Jito API unavailable | Automatic fallback to `sendTransaction` |
| Token API errors | Skip health check, proceed immediately |
| Configuration missing | Sensible defaults, clear warning logs |
| Performance impact | Async operations, caching for repeated queries |
| Breaking changes | All new code, minimal modifications to existing logic |

---

## Implementation Order

1. **Phase 1: Core Types** (Low Risk)
   - Add error variants to `error.rs`
   - Create `quicknode.rs` module skeleton
   - Create `privacy/` module structure

2. **Phase 2: Ghost Mode** (Medium Risk)
   - Implement `QuickNodePrivateSubmissionStrategy`
   - Integrate into `solana.rs`
   - Add fallback logic

3. **Phase 3: Privacy Health Check** (Medium Risk)
   - Implement `PrivacyHealthCheckService`
   - Integrate into `worker.rs`
   - Update `main.rs` initialization

4. **Phase 4: Testing & Polish** (Low Risk)
   - Add comprehensive tests
   - Documentation
   - Integration testing

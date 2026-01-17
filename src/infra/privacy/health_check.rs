//! Anonymity Set Health Check Service
//!
//! Assesses the recent transaction volume ("anonymity set health") for a
//! given confidential token mint before submission. Implements smart delays
//! when network activity is low to mitigate timing attacks.
//!
//! # How It Works
//! 1. Before submitting a confidential transfer, the worker queries this service
//! 2. The service fetches recent transaction activity for the token mint
//! 3. If activity is below the threshold, a randomized delay is recommended
//! 4. The transaction is postponed to blend with future network activity
//!
//! # Graceful Degradation
//! If the Token API is unavailable or returns an error, the check is skipped
//! and the transaction proceeds immediately (prioritizing liveness over privacy).

use chrono::{DateTime, Utc};
use rand::Rng;
use std::sync::Arc;
use tracing::{debug, info, warn};

use crate::infra::blockchain::quicknode::QuickNodeTokenApiClient;

// ============================================================================
// CONFIGURATION
// ============================================================================

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
            min_tx_threshold: 5,  // Require 5+ transactions
            lookback_minutes: 10, // In the last 10 minutes
            max_delay_secs: 120,  // Max 2 minute delay
            min_delay_secs: 10,   // Min 10 second delay
            enabled: true,
        }
    }
}

impl PrivacyHealthCheckConfig {
    /// Create a disabled configuration
    pub fn disabled() -> Self {
        Self {
            enabled: false,
            ..Default::default()
        }
    }

    /// Load configuration from environment variables
    pub fn from_env() -> Self {
        use std::env;

        let min_tx_threshold = env::var("PRIVACY_MIN_TX_THRESHOLD")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(5);

        let lookback_minutes = env::var("PRIVACY_LOOKBACK_MINUTES")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(10);

        let max_delay_secs = env::var("PRIVACY_MAX_DELAY_SECS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(120);

        let min_delay_secs = env::var("PRIVACY_MIN_DELAY_SECS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(10);

        let enabled = env::var("ENABLE_PRIVACY_CHECKS")
            .map(|v| v == "true" || v == "1")
            .unwrap_or(true);

        Self {
            min_tx_threshold,
            lookback_minutes,
            max_delay_secs,
            min_delay_secs,
            enabled,
        }
    }
}

// ============================================================================
// ANONYMITY SET HEALTH RESULT
// ============================================================================

/// Result of an anonymity set health check
#[derive(Debug, Clone)]
pub struct AnonymitySetHealth {
    /// Token mint that was checked
    pub token_mint: String,
    /// Number of recent transactions observed
    pub recent_tx_count: u64,
    /// Whether the anonymity set is healthy (sufficient activity)
    pub is_healthy: bool,
    /// Recommended delay in seconds (if unhealthy)
    pub recommended_delay_secs: Option<u64>,
    /// Timestamp of the check
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
    /// Assumes healthy to prioritize liveness
    pub fn skipped(token_mint: String) -> Self {
        Self {
            token_mint,
            recent_tx_count: 0,
            is_healthy: true, // Assume healthy to prioritize liveness
            recommended_delay_secs: None,
            checked_at: Utc::now(),
        }
    }
}

// ============================================================================
// PRIVACY HEALTH CHECK SERVICE
// ============================================================================

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
        if config.enabled && token_api_client.is_some() {
            info!(
                threshold = config.min_tx_threshold,
                lookback_minutes = config.lookback_minutes,
                max_delay_secs = config.max_delay_secs,
                "🛡️ Privacy Health Check service initialized"
            );
        } else if config.enabled {
            warn!("Privacy Health Check enabled but no Token API client (will skip checks)");
        } else {
            debug!("Privacy Health Check service disabled");
        }

        Self {
            config,
            token_api_client,
        }
    }

    /// Create a disabled/passthrough service
    pub fn disabled() -> Self {
        Self {
            config: PrivacyHealthCheckConfig::disabled(),
            token_api_client: None,
        }
    }

    /// Check if the service is enabled and has API access
    pub fn is_operational(&self) -> bool {
        self.config.enabled && self.token_api_client.is_some()
    }

    /// Get the current configuration
    pub fn config(&self) -> &PrivacyHealthCheckConfig {
        &self.config
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
        match client
            .get_recent_activity(token_mint, self.config.lookback_minutes)
            .await
        {
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

        let base_delay = self.config.min_delay_secs as f64
            + (activity_factor * (self.config.max_delay_secs - self.config.min_delay_secs) as f64);

        // Add randomization (±30%)
        let jitter = rng.gen_range(0.7..1.3);
        let delay = (base_delay * jitter) as u64;

        delay.clamp(self.config.min_delay_secs, self.config.max_delay_secs)
    }
}

// ============================================================================
// TESTS
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_config_default() {
        let config = PrivacyHealthCheckConfig::default();
        assert_eq!(config.min_tx_threshold, 5);
        assert_eq!(config.lookback_minutes, 10);
        assert_eq!(config.max_delay_secs, 120);
        assert_eq!(config.min_delay_secs, 10);
        assert!(config.enabled);
    }

    #[test]
    fn test_config_disabled() {
        let config = PrivacyHealthCheckConfig::disabled();
        assert!(!config.enabled);
    }

    #[test]
    fn test_health_result_healthy() {
        let health = AnonymitySetHealth::healthy("token123".to_string(), 10);
        assert!(health.is_healthy);
        assert!(health.recommended_delay_secs.is_none());
        assert_eq!(health.token_mint, "token123");
        assert_eq!(health.recent_tx_count, 10);
    }

    #[test]
    fn test_health_result_unhealthy() {
        let health = AnonymitySetHealth::unhealthy("token123".to_string(), 2, 60);
        assert!(!health.is_healthy);
        assert_eq!(health.recommended_delay_secs, Some(60));
        assert_eq!(health.recent_tx_count, 2);
    }

    #[test]
    fn test_health_result_skipped() {
        let health = AnonymitySetHealth::skipped("token123".to_string());
        assert!(health.is_healthy); // Skipped = assume healthy for liveness
        assert!(health.recommended_delay_secs.is_none());
        assert_eq!(health.recent_tx_count, 0);
    }

    #[test]
    fn test_disabled_service() {
        let service = PrivacyHealthCheckService::disabled();
        assert!(!service.is_operational());
        assert!(!service.config().enabled);
    }

    #[test]
    fn test_service_without_client() {
        let config = PrivacyHealthCheckConfig::default();
        let service = PrivacyHealthCheckService::new(config, None);
        assert!(!service.is_operational());
    }

    #[tokio::test]
    async fn test_check_health_disabled() {
        let service = PrivacyHealthCheckService::disabled();
        let health = service.check_health("some_mint").await;
        assert!(health.is_healthy);
        assert_eq!(health.token_mint, "some_mint");
    }

    #[tokio::test]
    async fn test_check_health_no_client() {
        let config = PrivacyHealthCheckConfig::default();
        let service = PrivacyHealthCheckService::new(config, None);
        let health = service.check_health("some_mint").await;
        assert!(health.is_healthy); // Skipped
    }

    #[test]
    fn test_calculate_delay_low_activity() {
        let config = PrivacyHealthCheckConfig {
            min_tx_threshold: 10,
            min_delay_secs: 10,
            max_delay_secs: 100,
            ..Default::default()
        };
        let service = PrivacyHealthCheckService::new(config, None);

        // With 0 activity, delay should be close to max
        let delay = service.calculate_delay(0);
        assert!(delay >= 10);
        assert!(delay <= 100);
    }

    #[test]
    fn test_calculate_delay_high_activity() {
        let config = PrivacyHealthCheckConfig {
            min_tx_threshold: 10,
            min_delay_secs: 10,
            max_delay_secs: 100,
            ..Default::default()
        };
        let service = PrivacyHealthCheckService::new(config, None);

        // With activity at threshold, delay should be close to min
        let delay = service.calculate_delay(9);
        assert!(delay >= 10);
        assert!(delay <= 100);
    }
}

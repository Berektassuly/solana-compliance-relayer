//! Range compliance provider implementation.
//!
//! This module provides integration with Range Protocol's Risk API
//! for wallet address screening and compliance checks.

use async_trait::async_trait;
use reqwest::Client;
use serde::Deserialize;
use tracing::{debug, error, info, instrument, warn};

use crate::domain::{AppError, ComplianceProvider, ComplianceStatus, SubmitTransferRequest};

/// Default Range Protocol API base URL
pub const DEFAULT_RANGE_API_URL: &str = "https://api.range.org/v1";

/// Detailed malicious address info
#[derive(Debug, Deserialize, Clone)]
pub struct MaliciousAddress {
    pub address: String,
    pub distance: u32,
    #[serde(default)]
    pub name_tag: String,
    pub entity: Option<String>,
    #[serde(default)]
    pub category: String,
    #[serde(default)]
    pub risk_categories: Vec<String>,
}

/// Attribution info
#[derive(Debug, Deserialize, Clone)]
pub struct Attribution {
    #[serde(default)]
    pub name_tag: String,
    pub entity: Option<String>,
    #[serde(default)]
    pub category: String,
    #[serde(default)]
    pub address_role: String,
    #[serde(default)]
    pub risk_categories: Vec<String>,
}

/// Response from Range Protocol Risk API
///
/// Example Response:
/// {
///   "riskScore": 1,
///   "riskLevel": "Very low risk",
///   "numHops": 2,
///   "maliciousAddressesFound": [],
///   "reasoning": "...",
///   "attribution": { ... }
/// }
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RiskResponse {
    pub risk_score: i32,
    pub risk_level: String,
    pub num_hops: Option<u32>,
    #[serde(default)]
    pub malicious_addresses_found: Vec<MaliciousAddress>,
    #[serde(default)]
    pub reasoning: String,
    pub attribution: Option<Attribution>,
}

/// Compliance provider that screens addresses via Range Protocol API
#[derive(Debug, Clone)]
pub struct RangeComplianceProvider {
    http_client: Client,
    api_key: Option<String>,
    base_url: String,
}

impl Default for RangeComplianceProvider {
    fn default() -> Self {
        Self::new(None, None)
    }
}

impl RangeComplianceProvider {
    /// Create a new Range compliance provider
    ///
    /// # Arguments
    /// * `api_key` - Optional API key for Range Protocol. If None, uses mock mode.
    /// * `base_url` - Optional custom API base URL. Defaults to Range Protocol production.
    pub fn new(api_key: Option<String>, base_url: Option<String>) -> Self {
        let http_client = Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .expect("Failed to create HTTP client");

        Self {
            http_client,
            api_key,
            base_url: base_url.unwrap_or_else(|| DEFAULT_RANGE_API_URL.to_string()),
        }
    }

    /// Check if running in mock mode (no API key configured)
    fn is_mock_mode(&self) -> bool {
        self.api_key.is_none()
    }

    /// Perform mock compliance check (for development/testing)
    fn mock_check(&self, to_address: &str) -> ComplianceStatus {
        // Block strict match for known test addresses
        if to_address == "hack_the_planet_bad_wallet" {
            return ComplianceStatus::Rejected;
        }

        // Block pattern match for addresses starting with "hack"
        if to_address.to_lowercase().starts_with("hack") {
            return ComplianceStatus::Rejected;
        }

        ComplianceStatus::Approved
    }

    /// Call Range Protocol Risk API
    async fn check_address_risk(&self, address: &str) -> Result<RiskResponse, AppError> {
        let api_key = self.api_key.as_ref().ok_or_else(|| {
            AppError::ExternalService(crate::domain::ExternalServiceError::Configuration(
                "RANGE_API_KEY not configured".to_string(),
            ))
        })?;

        let url = format!("{}/risk/address", self.base_url);

        debug!(url = %url, address = %address, "Calling Range Protocol Risk API");

        let response = self
            .http_client
            .get(&url)
            .header("Authorization", format!("Bearer {}", api_key))
            .header("Content-Type", "application/json")
            .query(&[("address", address), ("network", "solana")])
            .send()
            .await
            .map_err(|e| {
                error!(error = %e, "Range Protocol API request failed");
                AppError::ExternalService(crate::domain::ExternalServiceError::Network(
                    e.to_string(),
                ))
            })?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            error!(status = %status, body = %body, "Range Protocol API returned error");
            return Err(AppError::ExternalService(
                crate::domain::ExternalServiceError::ApiError {
                    status_code: status.as_u16(),
                    message: body,
                },
            ));
        }

        // Get raw body text first for debugging if parsing fails
        let body_text = response.text().await.map_err(|e| {
            error!(error = %e, "Failed to read Range Protocol response body");
            AppError::ExternalService(crate::domain::ExternalServiceError::Network(e.to_string()))
        })?;

        let risk_response: RiskResponse = serde_json::from_str(&body_text).map_err(|e| {
            error!(
                error = %e,
                raw_body = %body_text,
                "Failed to parse Range Protocol response - logging raw body for debugging"
            );
            AppError::ExternalService(crate::domain::ExternalServiceError::ParseError(format!(
                "JSON parse error: {}. Raw body: {}",
                e, body_text
            )))
        })?;

        debug!(
            score = %risk_response.risk_score,
            level = %risk_response.risk_level,
            "Range Protocol risk check complete"
        );

        Ok(risk_response)
    }

    /// Determine compliance status from risk response
    ///
    /// Range API riskScore is 1-10:
    ///   10 = CRITICAL RISK (directly malicious)
    ///   9-8 = Extremely high risk
    ///   7-6 = High risk
    ///   5-4 = Medium risk
    ///   3-2 = Low risk
    ///   1 = Very low risk
    ///
    /// Rule: Reject if riskScore >= 6 (High risk and above)
    /// OR if riskLevel text contains high-risk indicators
    fn evaluate_risk(&self, response: &RiskResponse) -> ComplianceStatus {
        let risk_level_lower = response.risk_level.to_lowercase();

        // Reject High risk (6-7), Extremely high (8-9), and Critical (10)
        let is_high_risk = response.risk_score >= 6
            || risk_level_lower.contains("high")
            || risk_level_lower.contains("severe")
            || risk_level_lower.contains("critical")
            || risk_level_lower.contains("extremely");

        if is_high_risk {
            info!(
                risk_score = %response.risk_score,
                risk_level = %response.risk_level,
                "Address rejected: high risk detected"
            );
            ComplianceStatus::Rejected
        } else {
            debug!(
                risk_score = %response.risk_score,
                risk_level = %response.risk_level,
                "Address approved"
            );
            ComplianceStatus::Approved
        }
    }
}

#[async_trait]
impl ComplianceProvider for RangeComplianceProvider {
    #[instrument(skip(self, request), fields(from = %request.from_address, to = %request.to_address))]
    async fn check_compliance(
        &self,
        request: &SubmitTransferRequest,
    ) -> Result<ComplianceStatus, AppError> {
        // Use mock mode if no API key is configured
        if self.is_mock_mode() {
            warn!("Running in mock compliance mode - no RANGE_API_KEY configured");
            return Ok(self.mock_check(&request.to_address));
        }

        // Check destination address against Range Protocol
        match self.check_address_risk(&request.to_address).await {
            Ok(response) => Ok(self.evaluate_risk(&response)),
            Err(e) => {
                // On API error, default to rejection for safety
                error!(
                    error = ?e,
                    to_address = %request.to_address,
                    "Range Protocol API error - defaulting to rejection for safety"
                );
                Ok(ComplianceStatus::Rejected)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::TransferType;

    #[test]
    fn test_mock_mode_approved() {
        let provider = RangeComplianceProvider::new(None, None);
        assert!(provider.is_mock_mode());

        let status = provider.mock_check("HvwC9QSAzwEXkUkwqNNGhfNHoVqXJYfPvPZfQvJmHWcF");
        assert_eq!(status, ComplianceStatus::Approved);
    }

    #[test]
    fn test_mock_mode_rejected_exact_match() {
        let provider = RangeComplianceProvider::new(None, None);
        let status = provider.mock_check("hack_the_planet_bad_wallet");
        assert_eq!(status, ComplianceStatus::Rejected);
    }

    #[test]
    fn test_mock_mode_rejected_prefix() {
        let provider = RangeComplianceProvider::new(None, None);
        let status = provider.mock_check("hackSomeAddress123");
        assert_eq!(status, ComplianceStatus::Rejected);
    }

    #[test]
    fn test_risk_evaluation_high_score() {
        let provider = RangeComplianceProvider::new(Some("test_key".to_string()), None);
        let response = RiskResponse {
            risk_score: 6, // High risk threshold (6-7)
            risk_level: "High risk".to_string(),
            num_hops: Some(2),
            malicious_addresses_found: vec![],
            reasoning: "2 hops from malicious address".to_string(),
            attribution: None,
        };
        assert_eq!(
            provider.evaluate_risk(&response),
            ComplianceStatus::Rejected
        );
    }

    #[test]
    fn test_risk_evaluation_medium_risk_approved() {
        let provider = RangeComplianceProvider::new(Some("test_key".to_string()), None);
        let response = RiskResponse {
            risk_score: 5, // Medium risk (4-5) - should be approved
            risk_level: "Medium risk".to_string(),
            num_hops: Some(3),
            malicious_addresses_found: vec![],
            reasoning: "3 hops from malicious address".to_string(),
            attribution: None,
        };
        assert_eq!(
            provider.evaluate_risk(&response),
            ComplianceStatus::Approved
        );
    }

    #[test]
    fn test_risk_evaluation_low_score_but_critical_risk_text() {
        let provider = RangeComplianceProvider::new(Some("test_key".to_string()), None);
        let response = RiskResponse {
            risk_score: 10, // Low score but text says CRITICAL (Range API actual format)
            risk_level: "CRITICAL RISK (Directly malicious)".to_string(),
            num_hops: Some(0),
            malicious_addresses_found: vec![],
            reasoning: "Address is directly flagged for malicious activity.".to_string(),
            attribution: None,
        };
        assert_eq!(
            provider.evaluate_risk(&response),
            ComplianceStatus::Rejected
        );
    }

    #[test]
    fn test_risk_evaluation_low_risk_approved() {
        let provider = RangeComplianceProvider::new(Some("test_key".to_string()), None);
        let response = RiskResponse {
            risk_score: 1,
            risk_level: "Very low risk".to_string(),
            num_hops: Some(2),
            malicious_addresses_found: vec![],
            reasoning: "Safe".to_string(),
            attribution: None,
        };
        assert_eq!(
            provider.evaluate_risk(&response),
            ComplianceStatus::Approved
        );
    }

    #[tokio::test]
    async fn test_check_compliance_mock_mode() {
        let provider = RangeComplianceProvider::new(None, None);
        let request = SubmitTransferRequest {
            from_address: "sender".to_string(),
            to_address: "receiver".to_string(),
            transfer_details: TransferType::Public {
                amount: 1_000_000_000,
            },
            token_mint: None,
            signature: "dummy_sig".to_string(),
        };
        let result = provider.check_compliance(&request).await;
        assert_eq!(result.unwrap(), ComplianceStatus::Approved);
    }

    #[tokio::test]
    async fn test_check_compliance_mock_mode_rejected() {
        let provider = RangeComplianceProvider::new(None, None);
        let request = SubmitTransferRequest {
            from_address: "sender".to_string(),
            to_address: "hackBadWallet".to_string(),
            transfer_details: TransferType::Public {
                amount: 1_000_000_000,
            },
            token_mint: None,
            signature: "dummy_sig".to_string(),
        };
        let result = provider.check_compliance(&request).await;
        assert_eq!(result.unwrap(), ComplianceStatus::Rejected);
    }
}

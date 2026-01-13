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
pub const DEFAULT_RANGE_API_URL: &str = "https://api.rangeprotocol.com/v1";

/// Risk score levels returned by Range Protocol
#[derive(Debug, Deserialize, Clone, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum RiskScore {
    Low,
    Medium,
    High,
    Severe,
    #[serde(other)]
    Unknown,
}

/// Response from Range Protocol Risk API
#[derive(Debug, Deserialize)]
pub struct RiskResponse {
    /// The wallet address that was checked
    pub address: String,
    /// Risk score classification
    pub risk_score: RiskScore,
    /// Whether the address appears on any sanctions list
    #[serde(default)]
    pub sanctions_list: bool,
    /// Optional additional risk details
    #[serde(default)]
    pub risk_factors: Vec<String>,
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

        let url = format!("{}/address/risk", self.base_url);

        debug!(url = %url, address = %address, "Calling Range Protocol Risk API");

        let response = self
            .http_client
            .get(&url)
            .header("Authorization", format!("Bearer {}", api_key))
            .header("Content-Type", "application/json")
            .query(&[("address", address)])
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

        let risk_response: RiskResponse = response.json().await.map_err(|e| {
            error!(error = %e, "Failed to parse Range Protocol response");
            AppError::ExternalService(crate::domain::ExternalServiceError::ParseError(
                e.to_string(),
            ))
        })?;

        debug!(
            address = %risk_response.address,
            risk_score = ?risk_response.risk_score,
            sanctions = %risk_response.sanctions_list,
            "Range Protocol risk check complete"
        );

        Ok(risk_response)
    }

    /// Determine compliance status from risk response
    fn evaluate_risk(&self, response: &RiskResponse) -> ComplianceStatus {
        // Reject if on sanctions list
        if response.sanctions_list {
            info!(
                address = %response.address,
                "Address rejected: on sanctions list"
            );
            return ComplianceStatus::Rejected;
        }

        // Reject if high or severe risk
        match response.risk_score {
            RiskScore::High | RiskScore::Severe => {
                info!(
                    address = %response.address,
                    risk_score = ?response.risk_score,
                    "Address rejected: high/severe risk score"
                );
                ComplianceStatus::Rejected
            }
            _ => {
                debug!(
                    address = %response.address,
                    risk_score = ?response.risk_score,
                    "Address approved"
                );
                ComplianceStatus::Approved
            }
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
    fn test_risk_evaluation_sanctions() {
        let provider = RangeComplianceProvider::new(Some("test_key".to_string()), None);
        let response = RiskResponse {
            address: "test".to_string(),
            risk_score: RiskScore::Low,
            sanctions_list: true,
            risk_factors: vec![],
        };
        assert_eq!(
            provider.evaluate_risk(&response),
            ComplianceStatus::Rejected
        );
    }

    #[test]
    fn test_risk_evaluation_high_risk() {
        let provider = RangeComplianceProvider::new(Some("test_key".to_string()), None);
        let response = RiskResponse {
            address: "test".to_string(),
            risk_score: RiskScore::High,
            sanctions_list: false,
            risk_factors: vec![],
        };
        assert_eq!(
            provider.evaluate_risk(&response),
            ComplianceStatus::Rejected
        );
    }

    #[test]
    fn test_risk_evaluation_severe_risk() {
        let provider = RangeComplianceProvider::new(Some("test_key".to_string()), None);
        let response = RiskResponse {
            address: "test".to_string(),
            risk_score: RiskScore::Severe,
            sanctions_list: false,
            risk_factors: vec![],
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
            address: "test".to_string(),
            risk_score: RiskScore::Low,
            sanctions_list: false,
            risk_factors: vec![],
        };
        assert_eq!(
            provider.evaluate_risk(&response),
            ComplianceStatus::Approved
        );
    }

    #[test]
    fn test_risk_evaluation_medium_risk_approved() {
        let provider = RangeComplianceProvider::new(Some("test_key".to_string()), None);
        let response = RiskResponse {
            address: "test".to_string(),
            risk_score: RiskScore::Medium,
            sanctions_list: false,
            risk_factors: vec![],
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
            amount_sol: 1.0,
            token_mint: None,
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
            amount_sol: 1.0,
            token_mint: None,
        };
        let result = provider.check_compliance(&request).await;
        assert_eq!(result.unwrap(), ComplianceStatus::Rejected);
    }
}

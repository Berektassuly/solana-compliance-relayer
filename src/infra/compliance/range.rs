//! Range compliance provider implementation.

use async_trait::async_trait;
use tracing::instrument;

use crate::domain::{AppError, ComplianceProvider, ComplianceStatus, SubmitTransferRequest};

/// Compliance provider that blocks suspicious addresses
#[derive(Debug, Clone, Default)]
pub struct RangeComplianceProvider;

impl RangeComplianceProvider {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl ComplianceProvider for RangeComplianceProvider {
    #[instrument(skip(self, request), fields(from = %request.from_address, to = %request.to_address))]
    async fn check_compliance(
        &self,
        request: &SubmitTransferRequest,
    ) -> Result<ComplianceStatus, AppError> {
        // Block strict match
        if request.to_address == "hack_the_planet_bad_wallet" {
            return Ok(ComplianceStatus::Rejected);
        }

        // Block pattern match
        if request.to_address.to_lowercase().starts_with("hack") {
            return Ok(ComplianceStatus::Rejected);
        }

        // Default approval
        Ok(ComplianceStatus::Approved)
    }
}

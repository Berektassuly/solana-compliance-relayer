//! Application state management.

use std::sync::Arc;

use crate::domain::{BlockchainClient, ComplianceProvider, DatabaseClient};
use crate::infra::BlocklistManager;
use crate::infra::privacy::PrivacyHealthCheckService;

use super::risk_service::RiskService;
use super::service::AppService;

/// Shared application state
#[derive(Clone)]
pub struct AppState {
    pub service: Arc<AppService>,
    pub db_client: Arc<dyn DatabaseClient>,
    pub blockchain_client: Arc<dyn BlockchainClient>,
    pub compliance_provider: Arc<dyn ComplianceProvider>,
    /// Helius webhook secret for authentication (optional)
    pub helius_webhook_secret: Option<String>,
    /// QuickNode webhook secret for authentication (optional)
    /// Used to validate incoming webhook payloads from QuickNode Streams
    pub quicknode_webhook_secret: Option<String>,
    /// Privacy health check service for confidential transfers
    pub privacy_service: Option<Arc<PrivacyHealthCheckService>>,
    /// Internal blocklist manager for local address screening
    pub blocklist: Option<Arc<BlocklistManager>>,
    /// Risk check service for pre-flight compliance screening
    pub risk_service: Option<Arc<RiskService>>,
}

impl AppState {
    /// Create a new application state
    #[must_use]
    pub fn new(
        db_client: Arc<dyn DatabaseClient>,
        blockchain_client: Arc<dyn BlockchainClient>,
        compliance_provider: Arc<dyn ComplianceProvider>,
    ) -> Self {
        Self::with_webhook_secrets(
            db_client,
            blockchain_client,
            compliance_provider,
            None,
            None,
        )
    }

    /// Create a new application state with Helius webhook secret
    #[must_use]
    pub fn with_helius_secret(
        db_client: Arc<dyn DatabaseClient>,
        blockchain_client: Arc<dyn BlockchainClient>,
        compliance_provider: Arc<dyn ComplianceProvider>,
        helius_webhook_secret: Option<String>,
    ) -> Self {
        Self::with_webhook_secrets(
            db_client,
            blockchain_client,
            compliance_provider,
            helius_webhook_secret,
            None,
        )
    }

    /// Create a new application state with both Helius and QuickNode webhook secrets
    #[must_use]
    pub fn with_webhook_secrets(
        db_client: Arc<dyn DatabaseClient>,
        blockchain_client: Arc<dyn BlockchainClient>,
        compliance_provider: Arc<dyn ComplianceProvider>,
        helius_webhook_secret: Option<String>,
        quicknode_webhook_secret: Option<String>,
    ) -> Self {
        let service = Arc::new(AppService::new(
            Arc::clone(&db_client),
            Arc::clone(&blockchain_client),
            Arc::clone(&compliance_provider),
        ));
        Self {
            service,
            db_client,
            blockchain_client,
            compliance_provider,
            helius_webhook_secret,
            quicknode_webhook_secret,
            privacy_service: None,
            blocklist: None,
            risk_service: None,
        }
    }

    /// Add privacy service to the application state (builder pattern)
    #[must_use]
    pub fn with_privacy_service(mut self, privacy_service: Arc<PrivacyHealthCheckService>) -> Self {
        self.privacy_service = Some(privacy_service);
        self
    }

    /// Add blocklist manager to the application state (builder pattern)
    /// This rebuilds the service to include blocklist integration
    #[must_use]
    pub fn with_blocklist(mut self, blocklist: Arc<BlocklistManager>) -> Self {
        // Rebuild the service with blocklist integration
        self.service = Arc::new(AppService::with_blocklist(
            Arc::clone(&self.db_client),
            Arc::clone(&self.blockchain_client),
            Arc::clone(&self.compliance_provider),
            Arc::clone(&blocklist),
        ));
        self.blocklist = Some(blocklist);
        self
    }

    /// Add risk service to the application state (builder pattern)
    #[must_use]
    pub fn with_risk_service(mut self, risk_service: Arc<RiskService>) -> Self {
        self.risk_service = Some(risk_service);
        self
    }
}

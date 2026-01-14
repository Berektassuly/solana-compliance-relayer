//! Application state management.

use std::sync::Arc;

use crate::domain::{BlockchainClient, DatabaseClient};

use super::service::AppService;

/// Shared application state
#[derive(Clone)]
pub struct AppState {
    pub service: Arc<AppService>,
    pub db_client: Arc<dyn DatabaseClient>,
    pub blockchain_client: Arc<dyn BlockchainClient>,
    /// Helius webhook secret for authentication (optional)
    pub helius_webhook_secret: Option<String>,
}

impl AppState {
    /// Create a new application state
    #[must_use]
    pub fn new(
        db_client: Arc<dyn DatabaseClient>,
        blockchain_client: Arc<dyn BlockchainClient>,
        compliance_provider: Arc<dyn crate::domain::ComplianceProvider>,
    ) -> Self {
        Self::with_helius_secret(db_client, blockchain_client, compliance_provider, None)
    }

    /// Create a new application state with Helius webhook secret
    #[must_use]
    pub fn with_helius_secret(
        db_client: Arc<dyn DatabaseClient>,
        blockchain_client: Arc<dyn BlockchainClient>,
        compliance_provider: Arc<dyn crate::domain::ComplianceProvider>,
        helius_webhook_secret: Option<String>,
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
            helius_webhook_secret,
        }
    }
}

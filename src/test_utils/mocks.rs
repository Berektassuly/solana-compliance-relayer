//! Mock implementations for testing.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use uuid::Uuid;

use crate::domain::{
    AppError, BlockchainClient, BlockchainError, BlockchainStatus, ComplianceStatus,
    DatabaseClient, DatabaseError, PaginatedResponse, SubmitTransferRequest, TransferRequest,
};

/// Configuration for mock behavior
#[derive(Debug, Clone, Default)]
pub struct MockConfig {
    pub should_fail: bool,
    pub error_message: Option<String>,
}

impl MockConfig {
    #[must_use]
    pub fn success() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn failure(message: impl Into<String>) -> Self {
        Self {
            should_fail: true,
            error_message: Some(message.into()),
        }
    }
}

/// Mock database client for testing
pub struct MockDatabaseClient {
    storage: Arc<Mutex<HashMap<String, TransferRequest>>>,
    config: MockConfig,
    is_healthy: AtomicBool,
}

impl MockDatabaseClient {
    #[must_use]
    pub fn new() -> Self {
        Self::with_config(MockConfig::success())
    }

    #[must_use]
    pub fn with_config(config: MockConfig) -> Self {
        Self {
            storage: Arc::new(Mutex::new(HashMap::new())),
            config,
            is_healthy: AtomicBool::new(true),
        }
    }

    #[must_use]
    pub fn failing(message: impl Into<String>) -> Self {
        Self::with_config(MockConfig::failure(message))
    }

    pub fn set_healthy(&self, healthy: bool) {
        self.is_healthy.store(healthy, Ordering::Relaxed);
    }

    /// Get all stored items (for testing)
    pub fn get_all_items(&self) -> Vec<TransferRequest> {
        self.storage.lock().unwrap().values().cloned().collect()
    }

    fn check_should_fail(&self) -> Result<(), AppError> {
        if self.config.should_fail {
            let msg = self
                .config
                .error_message
                .clone()
                .unwrap_or_else(|| "Mock error".to_string());
            return Err(AppError::Database(DatabaseError::Query(msg)));
        }
        Ok(())
    }
}

impl Default for MockDatabaseClient {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl DatabaseClient for MockDatabaseClient {
    async fn health_check(&self) -> Result<(), AppError> {
        if !self.is_healthy.load(Ordering::Relaxed) {
            return Err(AppError::Database(DatabaseError::Connection(
                "Unhealthy".to_string(),
            )));
        }
        self.check_should_fail()
    }

    async fn get_transfer_request(&self, id: &str) -> Result<Option<TransferRequest>, AppError> {
        self.check_should_fail()?;
        let storage = self.storage.lock().unwrap();
        Ok(storage.get(id).cloned())
    }

    async fn submit_transfer(&self, data: &SubmitTransferRequest) -> Result<TransferRequest, AppError> {
        self.check_should_fail()?;
        let id = Uuid::new_v4().to_string();
        let now = Utc::now();
        
        // Simulating compliance check (default to Pending or Mock logic)
        let compliance_status = ComplianceStatus::Pending; // Or Approved if we want to simulate auto-approve
        
        let request = TransferRequest {
            id: id.clone(),
            from_address: data.from_address.clone(),
            to_address: data.to_address.clone(),
            amount_sol: data.amount_sol,
            compliance_status,
            blockchain_status: BlockchainStatus::Pending,
            blockchain_signature: None,
            blockchain_retry_count: 0,
            blockchain_last_error: None,
            blockchain_next_retry_at: None,
            created_at: now,
            updated_at: now,
        };
        let mut storage = self.storage.lock().unwrap();
        storage.insert(id, request.clone());
        Ok(request)
    }

    async fn list_transfer_requests(
        &self,
        limit: i64,
        cursor: Option<&str>,
    ) -> Result<PaginatedResponse<TransferRequest>, AppError> {
        self.check_should_fail()?;
        let storage = self.storage.lock().unwrap();
        let mut items: Vec<TransferRequest> = storage.values().cloned().collect();
        items.sort_by(|a, b| b.created_at.cmp(&a.created_at));

        // Apply cursor
        let items = if let Some(cursor_id) = cursor {
            let pos = items.iter().position(|i| i.id == cursor_id);
            match pos {
                Some(p) => items.into_iter().skip(p + 1).collect(),
                None => {
                    return Err(AppError::Validation(
                        crate::domain::ValidationError::InvalidField {
                            field: "cursor".to_string(),
                            message: "Invalid cursor".to_string(),
                        },
                    ));
                }
            }
        } else {
            items
        };

        let limit = limit.clamp(1, 100) as usize;
        let has_more = items.len() > limit;
        let items: Vec<TransferRequest> = items.into_iter().take(limit).collect();
        let next_cursor = if has_more {
            items.last().map(|i| i.id.clone())
        } else {
            None
        };

        Ok(PaginatedResponse::new(items, next_cursor, has_more))
    }

    async fn update_blockchain_status(
        &self,
        id: &str,
        status: BlockchainStatus,
        signature: Option<&str>,
        error: Option<&str>,
        next_retry_at: Option<DateTime<Utc>>,
    ) -> Result<(), AppError> {
        self.check_should_fail()?;
        let mut storage = self.storage.lock().unwrap();
        if let Some(item) = storage.get_mut(id) {
            item.blockchain_status = status;
            if let Some(sig) = signature {
                item.blockchain_signature = Some(sig.to_string());
            }
            item.blockchain_last_error = error.map(|e| e.to_string());
            item.blockchain_next_retry_at = next_retry_at;
            item.updated_at = Utc::now();
        }
        Ok(())
    }

    async fn get_pending_blockchain_requests(&self, limit: i64) -> Result<Vec<TransferRequest>, AppError> {
        self.check_should_fail()?;
        let storage = self.storage.lock().unwrap();
        let now = Utc::now();
        let mut items: Vec<TransferRequest> = storage
            .values()
            .filter(|i| {
                i.blockchain_status == BlockchainStatus::PendingSubmission
                    && i.blockchain_retry_count < 10
                    && i.blockchain_next_retry_at.map(|t| t <= now).unwrap_or(true)
            })
            .cloned()
            .collect();
        items.sort_by(|a, b| a.created_at.cmp(&b.created_at));
        Ok(items.into_iter().take(limit as usize).collect())
    }

    async fn increment_retry_count(&self, id: &str) -> Result<i32, AppError> {
        self.check_should_fail()?;
        let mut storage = self.storage.lock().unwrap();
        if let Some(item) = storage.get_mut(id) {
            item.blockchain_retry_count += 1;
            item.updated_at = Utc::now();
            Ok(item.blockchain_retry_count)
        } else {
            Err(AppError::Database(DatabaseError::NotFound(id.to_string())))
        }
    }
}

/// Mock blockchain client for testing
pub struct MockBlockchainClient {
    transactions: Arc<Mutex<Vec<String>>>,
    config: MockConfig,
    is_healthy: AtomicBool,
}

impl MockBlockchainClient {
    #[must_use]
    pub fn new() -> Self {
        Self::with_config(MockConfig::success())
    }

    #[must_use]
    pub fn with_config(config: MockConfig) -> Self {
        Self {
            transactions: Arc::new(Mutex::new(Vec::new())),
            config,
            is_healthy: AtomicBool::new(true),
        }
    }

    #[must_use]
    pub fn failing(message: impl Into<String>) -> Self {
        Self::with_config(MockConfig::failure(message))
    }

    pub fn set_healthy(&self, healthy: bool) {
        self.is_healthy.store(healthy, Ordering::Relaxed);
    }

    pub fn get_transactions(&self) -> Vec<String> {
        self.transactions.lock().unwrap().clone()
    }

    fn check_should_fail(&self) -> Result<(), AppError> {
        if self.config.should_fail {
            let msg = self
                .config
                .error_message
                .clone()
                .unwrap_or_else(|| "Mock error".to_string());
            return Err(AppError::Blockchain(BlockchainError::TransactionFailed(
                msg,
            )));
        }
        Ok(())
    }
}

impl Default for MockBlockchainClient {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl BlockchainClient for MockBlockchainClient {
    async fn health_check(&self) -> Result<(), AppError> {
        if !self.is_healthy.load(Ordering::Relaxed) {
            return Err(AppError::Blockchain(BlockchainError::Connection(
                "Unhealthy".to_string(),
            )));
        }
        self.check_should_fail()
    }

    async fn submit_transaction(&self, request: &TransferRequest) -> Result<String, AppError> {
        self.check_should_fail()?;
        // Mock signature generation (e.g., hash of ID)
        let signature = format!("sig_{}", request.id);
        let mut transactions = self.transactions.lock().unwrap();
        transactions.push(request.id.clone());
        Ok(signature)
    }

    async fn get_transaction_status(&self, _signature: &str) -> Result<bool, AppError> {
        self.check_should_fail()?;
        // For mock purposes, assume if it's in our list it's valid
        // But here we store request IDs, not signatures. 
        // Let's simplified assumption: always true if not failing
        Ok(true)
    }



    async fn get_latest_blockhash(&self) -> Result<String, AppError> {
        self.check_should_fail()?;
        Ok("mock_blockhash_abc123".to_string())
    }
}

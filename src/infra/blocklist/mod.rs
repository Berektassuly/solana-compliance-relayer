//! Internal blocklist manager for local address screening.
//!
//! Provides a high-performance, thread-safe "hot cache" for blocking
//! malicious addresses before querying external compliance providers.
//! The blocklist is persisted to the database for durability across restarts.

use dashmap::DashMap;
use sqlx::PgPool;
use tracing::{info, warn};

use crate::domain::{AppError, DatabaseError};

/// Blocklist entry with address and reason
#[derive(Debug, Clone)]
pub struct BlocklistEntry {
    pub address: String,
    pub reason: String,
}

/// Thread-safe internal blocklist manager using DashMap for high-concurrency access.
///
/// Maps wallet addresses (String) to rejection reasons (String).
/// All changes are persisted to the database for durability.
#[derive(Debug)]
pub struct BlocklistManager {
    /// In-memory cache for O(1) lookups
    store: DashMap<String, String>,
    /// Database pool for persistence
    pool: PgPool,
}

impl BlocklistManager {
    /// Create a new BlocklistManager and load existing entries from database.
    ///
    /// # Arguments
    /// * `pool` - PostgreSQL connection pool for persistence
    ///
    /// # Returns
    /// A new BlocklistManager with entries loaded from the database
    pub async fn new(pool: PgPool) -> Result<Self, AppError> {
        let manager = Self {
            store: DashMap::new(),
            pool,
        };

        // Load existing blocklist entries from database
        manager.load_from_database().await?;

        info!(
            count = manager.store.len(),
            "BlocklistManager initialized from database"
        );

        Ok(manager)
    }

    /// Load all blocklist entries from the database into memory.
    async fn load_from_database(&self) -> Result<(), AppError> {
        let rows = sqlx::query_as::<_, (String, String)>(
            "SELECT address, reason FROM blocklist ORDER BY created_at"
        )
        .fetch_all(&self.pool)
        .await
        .map_err(|e| AppError::Database(DatabaseError::Query(e.to_string())))?;

        for (address, reason) in rows {
            self.store.insert(address, reason);
        }

        Ok(())
    }

    /// Check if an address is in the blocklist.
    /// Returns `Some(reason)` if blocked, `None` if not blocked.
    #[must_use]
    pub fn check_address(&self, address: &str) -> Option<String> {
        self.store.get(address).map(|entry| entry.value().clone())
    }

    /// Add or update an address in the blocklist.
    /// The change is persisted to the database.
    pub async fn add_address(&self, address: String, reason: String) -> Result<(), AppError> {
        let is_update = self.store.contains_key(&address);

        // Persist to database first (upsert)
        sqlx::query(
            r#"
            INSERT INTO blocklist (address, reason, created_at, updated_at)
            VALUES ($1, $2, NOW(), NOW())
            ON CONFLICT (address) DO UPDATE SET
                reason = EXCLUDED.reason,
                updated_at = NOW()
            "#,
        )
        .bind(&address)
        .bind(&reason)
        .execute(&self.pool)
        .await
        .map_err(|e| AppError::Database(DatabaseError::Query(e.to_string())))?;

        // Update in-memory cache
        self.store.insert(address.clone(), reason.clone());

        if is_update {
            info!(
                address = %address,
                reason = %reason,
                "Blocklist entry updated"
            );
        } else {
            warn!(
                address = %address,
                reason = %reason,
                "Address added to blocklist"
            );
        }

        Ok(())
    }

    /// Remove an address from the blocklist.
    /// The change is persisted to the database.
    /// Returns `true` if the address was present and removed.
    pub async fn remove_address(&self, address: &str) -> Result<bool, AppError> {
        // Remove from database first
        let result = sqlx::query("DELETE FROM blocklist WHERE address = $1")
            .bind(address)
            .execute(&self.pool)
            .await
            .map_err(|e| AppError::Database(DatabaseError::Query(e.to_string())))?;

        // Remove from in-memory cache
        if let Some((addr, _reason)) = self.store.remove(address) {
            warn!(
                address = %addr,
                "Address removed from blocklist"
            );
            Ok(true)
        } else if result.rows_affected() > 0 {
            // Edge case: was in DB but not in memory (shouldn't happen normally)
            warn!(
                address = %address,
                "Address removed from blocklist (was not in cache)"
            );
            Ok(true)
        } else {
            Ok(false)
        }
    }

    /// Get the current number of blocklisted addresses.
    #[must_use]
    pub fn len(&self) -> usize {
        self.store.len()
    }

    /// Check if the blocklist is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.store.is_empty()
    }

    /// List all blocklisted addresses (for admin purposes).
    #[must_use]
    pub fn list_all(&self) -> Vec<BlocklistEntry> {
        self.store
            .iter()
            .map(|entry| BlocklistEntry {
                address: entry.key().clone(),
                reason: entry.value().clone(),
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Note: Full integration tests require a PostgreSQL database.
    // Unit tests for the in-memory operations can be done with mocks.

    #[test]
    fn test_blocklist_entry_debug() {
        let entry = BlocklistEntry {
            address: "test_addr".to_string(),
            reason: "test_reason".to_string(),
        };
        let debug_str = format!("{:?}", entry);
        assert!(debug_str.contains("test_addr"));
        assert!(debug_str.contains("test_reason"));
    }
}

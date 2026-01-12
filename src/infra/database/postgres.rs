//! PostgreSQL database client implementation.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use sqlx::{PgPool, Row, postgres::PgPoolOptions};
use std::time::Duration;
use tracing::{info, instrument};

use crate::domain::{
    AppError, BlockchainStatus, ComplianceStatus, DatabaseClient, DatabaseError, PaginatedResponse,
    SubmitTransferRequest, TransferRequest,
};

/// PostgreSQL connection pool configuration
#[derive(Debug, Clone)]
pub struct PostgresConfig {
    pub max_connections: u32,
    pub min_connections: u32,
    pub acquire_timeout: Duration,
    pub idle_timeout: Duration,
    pub max_lifetime: Duration,
}

impl Default for PostgresConfig {
    fn default() -> Self {
        Self {
            max_connections: 10,
            min_connections: 2,
            acquire_timeout: Duration::from_secs(3),
            idle_timeout: Duration::from_secs(600),
            max_lifetime: Duration::from_secs(1800),
        }
    }
}

/// PostgreSQL database client with connection pooling
pub struct PostgresClient {
    pool: PgPool,
}

impl PostgresClient {
    /// Create a new PostgreSQL client with custom configuration
    pub async fn new(database_url: &str, config: PostgresConfig) -> Result<Self, AppError> {
        info!("Connecting to PostgreSQL...");
        let pool = PgPoolOptions::new()
            .max_connections(config.max_connections)
            .min_connections(config.min_connections)
            .acquire_timeout(config.acquire_timeout)
            .idle_timeout(config.idle_timeout)
            .max_lifetime(config.max_lifetime)
            .connect(database_url)
            .await
            .map_err(|e| AppError::Database(DatabaseError::Connection(e.to_string())))?;
        info!("Connected to PostgreSQL");
        Ok(Self { pool })
    }

    /// Create a new PostgreSQL client with default configuration
    pub async fn with_defaults(database_url: &str) -> Result<Self, AppError> {
        Self::new(database_url, PostgresConfig::default()).await
    }

    /// Run database migrations using sqlx migrate
    pub async fn run_migrations(&self) -> Result<(), AppError> {
        info!("Running database migrations...");
        sqlx::migrate!("./migrations")
            .run(&self.pool)
            .await
            .map_err(|e| AppError::Database(DatabaseError::Migration(e.to_string())))?;
        info!("Database migrations completed successfully");
        Ok(())
    }

    /// Get the underlying connection pool (for testing)
    #[must_use]
    pub fn pool(&self) -> &PgPool {
        &self.pool
    }

    /// Parse a database row into a TransferRequest
    fn row_to_transfer_request(row: &sqlx::postgres::PgRow) -> Result<TransferRequest, AppError> {
        let compliance_status_str: String = row.get("compliance_status");
        let blockchain_status_str: String = row.get("blockchain_status");

        Ok(TransferRequest {
            id: row.get("id"),
            from_address: row.get("from_address"),
            to_address: row.get("to_address"),
            amount_sol: row.get("amount_sol"),
            compliance_status: compliance_status_str
                .parse()
                .unwrap_or(ComplianceStatus::Pending),
            blockchain_status: blockchain_status_str
                .parse()
                .unwrap_or(BlockchainStatus::Pending),
            blockchain_signature: row.get("blockchain_signature"),
            blockchain_retry_count: row.get("blockchain_retry_count"),
            blockchain_last_error: row.get("blockchain_last_error"),
            blockchain_next_retry_at: row.get("blockchain_next_retry_at"),
            created_at: row.get("created_at"),
            updated_at: row.get("updated_at"),
        })
    }
}

#[async_trait]
impl DatabaseClient for PostgresClient {
    #[instrument(skip(self))]
    async fn health_check(&self) -> Result<(), AppError> {
        sqlx::query("SELECT 1")
            .execute(&self.pool)
            .await
            .map_err(|e| AppError::Database(DatabaseError::Connection(e.to_string())))?;
        Ok(())
    }

    #[instrument(skip(self))]
    async fn get_transfer_request(&self, id: &str) -> Result<Option<TransferRequest>, AppError> {
        let row = sqlx::query(
            r#"
            SELECT id, from_address, to_address, amount_sol, compliance_status,
                   blockchain_status, blockchain_signature, blockchain_retry_count,
                   blockchain_last_error, blockchain_next_retry_at,
                   created_at, updated_at 
            FROM transfer_requests 
            WHERE id = $1
            "#,
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| AppError::Database(DatabaseError::Query(e.to_string())))?;

        match row {
            Some(row) => Ok(Some(Self::row_to_transfer_request(&row)?)),
            None => Ok(None),
        }
    }

    #[instrument(skip(self, data), fields(from = %data.from_address, to = %data.to_address, amount = %data.amount_sol))]
    async fn submit_transfer(
        &self,
        data: &SubmitTransferRequest,
    ) -> Result<TransferRequest, AppError> {
        let id = uuid::Uuid::new_v4().to_string();
        let now = Utc::now();

        sqlx::query(
            r#"
            INSERT INTO transfer_requests (
                id, from_address, to_address, amount_sol, 
                compliance_status, blockchain_status, blockchain_retry_count,
                created_at, updated_at
            ) 
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
            "#,
        )
        .bind(&id)
        .bind(&data.from_address)
        .bind(&data.to_address)
        .bind(data.amount_sol)
        .bind(ComplianceStatus::Pending.as_str())
        .bind(BlockchainStatus::Pending.as_str())
        .bind(0i32)
        .bind(now)
        .bind(now)
        .execute(&self.pool)
        .await
        .map_err(|e| AppError::Database(DatabaseError::from(e)))?;

        Ok(TransferRequest {
            id,
            from_address: data.from_address.clone(),
            to_address: data.to_address.clone(),
            amount_sol: data.amount_sol,
            compliance_status: ComplianceStatus::Pending,
            blockchain_status: BlockchainStatus::Pending,
            blockchain_signature: None,
            blockchain_retry_count: 0,
            blockchain_last_error: None,
            blockchain_next_retry_at: None,
            created_at: now,
            updated_at: now,
        })
    }

    #[instrument(skip(self))]
    async fn list_transfer_requests(
        &self,
        limit: i64,
        cursor: Option<&str>,
    ) -> Result<PaginatedResponse<TransferRequest>, AppError> {
        // Clamp limit to valid range
        let limit = limit.clamp(1, 100);
        // Fetch one extra to determine if there are more items
        let fetch_limit = limit + 1;

        let rows = match cursor {
            Some(cursor_id) => {
                // Get the created_at of the cursor item for proper pagination
                let cursor_row =
                    sqlx::query("SELECT created_at FROM transfer_requests WHERE id = $1")
                        .bind(cursor_id)
                        .fetch_optional(&self.pool)
                        .await
                        .map_err(|e| AppError::Database(DatabaseError::Query(e.to_string())))?;

                let cursor_created_at: DateTime<Utc> = match cursor_row {
                    Some(row) => row.get("created_at"),
                    None => {
                        return Err(AppError::Validation(
                            crate::domain::ValidationError::InvalidField {
                                field: "cursor".to_string(),
                                message: "Invalid cursor".to_string(),
                            },
                        ));
                    }
                };

                sqlx::query(
                    r#"
                    SELECT id, from_address, to_address, amount_sol, compliance_status,
                           blockchain_status, blockchain_signature, blockchain_retry_count,
                           blockchain_last_error, blockchain_next_retry_at,
                           created_at, updated_at
                    FROM transfer_requests
                    WHERE (created_at, id) < ($1, $2)
                    ORDER BY created_at DESC, id DESC
                    LIMIT $3
                    "#,
                )
                .bind(cursor_created_at)
                .bind(cursor_id)
                .bind(fetch_limit)
                .fetch_all(&self.pool)
                .await
                .map_err(|e| AppError::Database(DatabaseError::Query(e.to_string())))?
            }
            None => sqlx::query(
                r#"
                    SELECT id, from_address, to_address, amount_sol, compliance_status,
                           blockchain_status, blockchain_signature, blockchain_retry_count,
                           blockchain_last_error, blockchain_next_retry_at,
                           created_at, updated_at
                    FROM transfer_requests
                    ORDER BY created_at DESC, id DESC
                    LIMIT $1
                    "#,
            )
            .bind(fetch_limit)
            .fetch_all(&self.pool)
            .await
            .map_err(|e| AppError::Database(DatabaseError::Query(e.to_string())))?,
        };

        let has_more = rows.len() > limit as usize;
        let requests: Vec<TransferRequest> = rows
            .iter()
            .take(limit as usize)
            .map(Self::row_to_transfer_request)
            .collect::<Result<Vec<_>, _>>()?;

        let next_cursor = if has_more {
            requests.last().map(|req| req.id.clone())
        } else {
            None
        };

        Ok(PaginatedResponse::new(requests, next_cursor, has_more))
    }

    #[instrument(skip(self))]
    async fn update_blockchain_status(
        &self,
        id: &str,
        status: BlockchainStatus,
        signature: Option<&str>,
        error: Option<&str>,
        next_retry_at: Option<DateTime<Utc>>,
    ) -> Result<(), AppError> {
        let now = Utc::now();

        sqlx::query(
            r#"
            UPDATE transfer_requests 
            SET blockchain_status = $1,
                blockchain_signature = COALESCE($2, blockchain_signature),
                blockchain_last_error = $3,
                blockchain_next_retry_at = $4,
                updated_at = $5
            WHERE id = $6
            "#,
        )
        .bind(status.as_str())
        .bind(signature)
        .bind(error)
        .bind(next_retry_at)
        .bind(now)
        .bind(id)
        .execute(&self.pool)
        .await
        .map_err(|e| AppError::Database(DatabaseError::Query(e.to_string())))?;

        Ok(())
    }

    #[instrument(skip(self))]
    async fn get_pending_blockchain_requests(
        &self,
        limit: i64,
    ) -> Result<Vec<TransferRequest>, AppError> {
        let now = Utc::now();
        let rows = sqlx::query(
            r#"
            SELECT id, from_address, to_address, amount_sol, compliance_status,
                   blockchain_status, blockchain_signature, blockchain_retry_count,
                   blockchain_last_error, blockchain_next_retry_at,
                   created_at, updated_at
            FROM transfer_requests
            WHERE blockchain_status = 'pending_submission'
              AND (blockchain_next_retry_at IS NULL OR blockchain_next_retry_at <= $1)
              AND blockchain_retry_count < 10
            ORDER BY blockchain_next_retry_at ASC NULLS FIRST, created_at ASC
            LIMIT $2
            "#,
        )
        .bind(now)
        .bind(limit)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| AppError::Database(DatabaseError::Query(e.to_string())))?;

        rows.iter().map(Self::row_to_transfer_request).collect()
    }

    #[instrument(skip(self))]
    async fn increment_retry_count(&self, id: &str) -> Result<i32, AppError> {
        let row = sqlx::query(
            r#"
            UPDATE transfer_requests 
            SET blockchain_retry_count = blockchain_retry_count + 1,
                updated_at = NOW()
            WHERE id = $1
            RETURNING blockchain_retry_count
            "#,
        )
        .bind(id)
        .fetch_one(&self.pool)
        .await
        .map_err(|e| AppError::Database(DatabaseError::Query(e.to_string())))?;

        Ok(row.get("blockchain_retry_count"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_postgres_config_default() {
        let config = PostgresConfig::default();
        assert_eq!(config.max_connections, 10);
        assert_eq!(config.min_connections, 2);
        assert_eq!(config.acquire_timeout, Duration::from_secs(3));
        assert_eq!(config.idle_timeout, Duration::from_secs(600));
        assert_eq!(config.max_lifetime, Duration::from_secs(1800));
    }

    #[test]
    fn test_postgres_config_custom() {
        let config = PostgresConfig {
            max_connections: 20,
            min_connections: 5,
            acquire_timeout: Duration::from_secs(10),
            idle_timeout: Duration::from_secs(300),
            max_lifetime: Duration::from_secs(3600),
        };
        assert_eq!(config.max_connections, 20);
        assert_eq!(config.min_connections, 5);
        assert_eq!(config.acquire_timeout, Duration::from_secs(10));
        assert_eq!(config.idle_timeout, Duration::from_secs(300));
        assert_eq!(config.max_lifetime, Duration::from_secs(3600));
    }
}

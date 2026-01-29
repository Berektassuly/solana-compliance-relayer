//! Database integration tests using testcontainers.
//!
//! These tests require Docker to be running and use testcontainers
//! to spin up a real PostgreSQL instance.

use testcontainers::{GenericImage, ImageExt, runners::AsyncRunner};

use solana_compliance_relayer::domain::{
    BlockchainStatus, DatabaseClient, SubmitTransferRequest, TransferType,
};
use solana_compliance_relayer::infra::{PostgresClient, PostgresConfig};

/// Helper to create a PostgreSQL container and client
async fn setup_postgres() -> (PostgresClient, testcontainers::ContainerAsync<GenericImage>) {
    let container = GenericImage::new("postgres", "16-alpine")
        .with_env_var("POSTGRES_DB", "test_db")
        .with_env_var("POSTGRES_USER", "postgres")
        .with_env_var("POSTGRES_PASSWORD", "postgres")
        .start()
        .await
        .expect("Failed to start postgres container");

    let port = container
        .get_host_port_ipv4(5432)
        .await
        .expect("Failed to get postgres port");

    let database_url = format!("postgres://postgres:postgres@127.0.0.1:{}/test_db", port);

    // Wait for postgres to be ready
    let mut attempts = 0;
    let client = loop {
        attempts += 1;
        match PostgresClient::new(&database_url, PostgresConfig::default()).await {
            Ok(client) => break client,
            Err(_) if attempts < 30 => {
                tokio::time::sleep(std::time::Duration::from_millis(500)).await;
            }
            Err(e) => panic!("Failed to connect to postgres after 30 attempts: {:?}", e),
        }
    };

    // Run migrations
    client
        .run_migrations()
        .await
        .expect("Failed to run migrations");

    (client, container)
}

#[tokio::test]
async fn test_create_and_get_transfer_request() {
    let (client, _container) = setup_postgres().await;

    let request = SubmitTransferRequest {
        from_address: "From1".to_string(),
        to_address: "To1".to_string(),
        transfer_details: TransferType::Public {
            amount: 100_000_000_000,
        },
        token_mint: None,
        signature: "dummy_sig".to_string(),
        nonce: "019470a4-7e7c-7d3e-8f1a-2b3c4d5e6001".to_string(),
    };

    // Create item
    let created = client
        .submit_transfer(&request)
        .await
        .expect("Failed to submit transfer");
    assert_eq!(created.from_address, "From1");
    assert_eq!(created.to_address, "To1");
    assert_eq!(
        created.transfer_details,
        TransferType::Public {
            amount: 100_000_000_000
        }
    );
    assert!(!created.id.is_empty());

    // Get item
    let fetched = client
        .get_transfer_request(&created.id)
        .await
        .expect("Failed to get request")
        .expect("Request not found");

    assert_eq!(fetched.id, created.id);
    assert_eq!(fetched.from_address, created.from_address);
}

#[tokio::test]
async fn test_list_requests_pagination() {
    let (client, _container) = setup_postgres().await;

    // Create 5 items
    for i in 0..5 {
        let request = SubmitTransferRequest {
            from_address: format!("From{}", i),
            to_address: format!("To{}", i),
            transfer_details: TransferType::Public {
                amount: (i as u64) * 1_000_000_000,
            },
            token_mint: None,
            signature: "dummy_sig".to_string(),
            nonce: format!("019470a4-7e7c-7d3e-8f1a-2b3c4d5e60{:02}", i),
        };
        client
            .submit_transfer(&request)
            .await
            .expect("Failed to submit transfer");
        // Small delay to ensure different timestamps
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }

    // Get first page (limit 2)
    let page1 = client
        .list_transfer_requests(2, None)
        .await
        .expect("Failed to list requests");
    assert_eq!(page1.items.len(), 2);
    assert!(page1.has_more);
    assert!(page1.next_cursor.is_some());

    // Get second page
    let page2 = client
        .list_transfer_requests(2, page1.next_cursor.as_deref())
        .await
        .expect("Failed to list requests");
    assert_eq!(page2.items.len(), 2);
    assert!(page2.has_more);

    // Get third page
    let page3 = client
        .list_transfer_requests(2, page2.next_cursor.as_deref())
        .await
        .expect("Failed to list requests");
    assert_eq!(page3.items.len(), 1);
    assert!(!page3.has_more);
    assert!(page3.next_cursor.is_none());
}

#[tokio::test]
async fn test_blockchain_status_updates() {
    let (client, _container) = setup_postgres().await;

    let request = SubmitTransferRequest {
        from_address: "From".to_string(),
        to_address: "To".to_string(),
        transfer_details: TransferType::Public {
            amount: 1_000_000_000,
        },
        token_mint: None,
        signature: "dummy_sig".to_string(),
        nonce: "019470a4-7e7c-7d3e-8f1a-2b3c4d5e6100".to_string(),
    };
    let created = client
        .submit_transfer(&request)
        .await
        .expect("Failed to submit transfer");
    assert_eq!(created.blockchain_status, BlockchainStatus::Pending);

    // Update to pending submission
    client
        .update_blockchain_status(
            &created.id,
            BlockchainStatus::PendingSubmission,
            None,
            Some("Initial error"),
            Some(chrono::Utc::now()),
        )
        .await
        .expect("Failed to update status");

    let fetched = client
        .get_transfer_request(&created.id)
        .await
        .expect("Failed to get request")
        .expect("Request not found");
    assert_eq!(
        fetched.blockchain_status,
        BlockchainStatus::PendingSubmission
    );
    assert_eq!(
        fetched.blockchain_last_error,
        Some("Initial error".to_string())
    );

    // Update to submitted
    client
        .update_blockchain_status(
            &created.id,
            BlockchainStatus::Submitted,
            Some("signature123"),
            None,
            None,
        )
        .await
        .expect("Failed to update status");

    let fetched = client
        .get_transfer_request(&created.id)
        .await
        .expect("Failed to get request")
        .expect("Request not found");
    assert_eq!(fetched.blockchain_status, BlockchainStatus::Submitted);
    assert_eq!(
        fetched.blockchain_signature,
        Some("signature123".to_string())
    );
}

#[tokio::test]
async fn test_get_pending_blockchain_requests() {
    let (client, _container) = setup_postgres().await;

    // Create items with different statuses
    for i in 0..3 {
        let request = SubmitTransferRequest {
            from_address: format!("From{}", i),
            to_address: format!("To{}", i),
            transfer_details: TransferType::Public {
                amount: 1_000_000_000,
            },
            token_mint: None,
            signature: "dummy_sig".to_string(),
            nonce: format!("019470a4-7e7c-7d3e-8f1a-2b3c4d5e62{:02}", i),
        };
        let item = client
            .submit_transfer(&request)
            .await
            .expect("Failed to submit transfer");

        if i == 0 {
            // Leave as pending
        } else if i == 1 {
            // Set to pending_submission AND approved (query requires both)
            client
                .update_compliance_status(
                    &item.id,
                    solana_compliance_relayer::domain::ComplianceStatus::Approved,
                )
                .await
                .expect("Failed to update compliance status");
            client
                .update_blockchain_status(
                    &item.id,
                    BlockchainStatus::PendingSubmission,
                    None,
                    None,
                    None,
                )
                .await
                .expect("Failed to update status");
        } else {
            // Set to confirmed
            client
                .update_blockchain_status(
                    &item.id,
                    BlockchainStatus::Confirmed,
                    Some("sig"),
                    None,
                    None,
                )
                .await
                .expect("Failed to update status");
        }
    }

    let pending = client
        .get_pending_blockchain_requests(10)
        .await
        .expect("Failed to get pending requests");

    // Only the item with pending_submission status should be returned
    // NOTE: The atomic claim logic updates status to Processing when fetching
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].blockchain_status, BlockchainStatus::Processing);
}

#[tokio::test]
async fn test_increment_retry_count() {
    let (client, _container) = setup_postgres().await;

    let request = SubmitTransferRequest {
        from_address: "From".to_string(),
        to_address: "To".to_string(),
        transfer_details: TransferType::Public {
            amount: 1_000_000_000,
        },
        token_mint: None,
        signature: "dummy_sig".to_string(),
        nonce: "019470a4-7e7c-7d3e-8f1a-2b3c4d5e6300".to_string(),
    };
    let created = client
        .submit_transfer(&request)
        .await
        .expect("Failed to submit transfer");
    assert_eq!(created.blockchain_retry_count, 0);

    // Increment retry count
    let count1 = client
        .increment_retry_count(&created.id)
        .await
        .expect("Failed to increment");
    assert_eq!(count1, 1);

    let count2 = client
        .increment_retry_count(&created.id)
        .await
        .expect("Failed to increment");
    assert_eq!(count2, 2);

    // Verify in database
    let fetched = client
        .get_transfer_request(&created.id)
        .await
        .expect("Failed to get request")
        .expect("Request not found");
    assert_eq!(fetched.blockchain_retry_count, 2);
}

#[tokio::test]
async fn test_health_check() {
    let (client, _container) = setup_postgres().await;

    let result = client.health_check().await;
    assert!(result.is_ok());
}

#[tokio::test]
async fn test_get_nonexistent_request() {
    let (client, _container) = setup_postgres().await;

    let result = client
        .get_transfer_request("nonexistent_id")
        .await
        .expect("Query should succeed");
    assert!(result.is_none());
}

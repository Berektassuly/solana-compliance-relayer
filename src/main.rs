//! Application entry point.

use std::env;
use std::sync::Arc;

use anyhow::{Context, Result};
use dotenvy::dotenv;
use ed25519_dalek::SigningKey;
use secrecy::SecretString;
use tokio::signal;
use tracing::{info, warn};
use tracing_subscriber::{EnvFilter, layer::SubscriberExt, util::SubscriberInitExt};

use solana_compliance_relayer::api::{
    RateLimitConfig, create_router, create_router_with_rate_limit,
};
use solana_compliance_relayer::app::{
    AppState, CrankConfig, RiskService, WorkerConfig, spawn_crank, spawn_worker,
    spawn_worker_with_privacy,
};
use solana_compliance_relayer::infra::RpcBlockchainClient;
use solana_compliance_relayer::infra::blockchain::{
    QuickNodePrivateSubmissionStrategy, QuickNodeSubmissionConfig, QuickNodeTokenApiClient,
    RpcProviderType,
};
use solana_compliance_relayer::infra::compliance::range::DEFAULT_RISK_THRESHOLD;
use solana_compliance_relayer::infra::{
    BlocklistManager, PostgresClient, PostgresConfig, PrivacyHealthCheckConfig,
    PrivacyHealthCheckService, signing_key_from_base58,
};

/// Application configuration
struct Config {
    database_url: String,
    blockchain_rpc_url: String,
    signing_key: SigningKey,
    host: String,
    port: u16,
    enable_rate_limiting: bool,
    rate_limit_config: RateLimitConfig,
    enable_background_worker: bool,
    worker_config: WorkerConfig,
    /// Range Protocol API key (optional - uses mock mode if not set)
    range_api_key: Option<String>,
    /// Range Protocol API base URL (optional - uses default if not set)
    range_api_url: Option<String>,
    /// Risk threshold for Range compliance (default: 6 = High Risk)
    range_risk_threshold: i32,
    /// Helius webhook secret for authentication (optional)
    helius_webhook_secret: Option<String>,
    /// QuickNode webhook secret for authentication (optional)
    quicknode_webhook_secret: Option<String>,
    /// Enable privacy health checks for confidential transfers
    enable_privacy_checks: bool,
    /// Enable Jito bundle submission for MEV-protected transactions (QuickNode only)
    /// When enabled, transactions are submitted privately via Jito block builders,
    /// bypassing the public mempool.
    use_jito_bundles: bool,
    /// Jito tip amount in lamports (default: 1000 = 0.000001 SOL)
    jito_tip_lamports: u64,
    /// Enable stale transaction crank (active polling fallback for webhook failures)
    enable_stale_crank: bool,
    /// Crank poll interval in seconds (default: 60)
    crank_poll_interval_secs: u64,
    /// Consider transactions stale after this many seconds (default: 90)
    crank_stale_after_secs: i64,
    /// Number of stale transactions to process per crank cycle (default: 20)
    crank_batch_size: i64,
}

impl Config {
    fn from_env() -> Result<Self> {
        let database_url = env::var("DATABASE_URL").context("DATABASE_URL not set")?;
        let blockchain_rpc_url = env::var("SOLANA_RPC_URL")
            .unwrap_or_else(|_| "https://api.devnet.solana.com".to_string());
        let signing_key = Self::load_signing_key()?;
        let host = env::var("HOST").unwrap_or_else(|_| "0.0.0.0".to_string());
        let port = env::var("PORT")
            .ok()
            .and_then(|p| p.parse().ok())
            .unwrap_or(3000);
        let enable_rate_limiting = env::var("ENABLE_RATE_LIMITING")
            .map(|v| v == "true" || v == "1")
            .unwrap_or(false);
        let enable_background_worker = env::var("ENABLE_BACKGROUND_WORKER")
            .map(|v| v == "true" || v == "1")
            .unwrap_or(true);

        // Range Protocol configuration (optional)
        let range_api_key = env::var("RANGE_API_KEY").ok().filter(|k| !k.is_empty());
        let range_api_url = env::var("RANGE_API_URL").ok().filter(|u| !u.is_empty());

        // Helius webhook configuration (optional)
        let helius_webhook_secret = env::var("HELIUS_WEBHOOK_SECRET")
            .ok()
            .filter(|s| !s.is_empty());

        // QuickNode webhook configuration (optional)
        let quicknode_webhook_secret = env::var("QUICKNODE_WEBHOOK_SECRET")
            .ok()
            .filter(|s| !s.is_empty());

        // Range risk threshold configuration
        let range_risk_threshold = env::var("RANGE_RISK_THRESHOLD")
            .ok()
            .and_then(|v| v.parse::<i32>().ok())
            .unwrap_or(DEFAULT_RISK_THRESHOLD);

        let rate_limit_config = RateLimitConfig::from_env();

        // Privacy health check configuration
        let enable_privacy_checks = env::var("ENABLE_PRIVACY_CHECKS")
            .map(|v| v == "true" || v == "1")
            .unwrap_or(true); // Enabled by default

        // Jito bundle configuration (QuickNode only)
        // Default: false for safety - explicit opt-in required
        let use_jito_bundles = env::var("USE_JITO_BUNDLES")
            .map(|v| v == "true" || v == "1")
            .unwrap_or(false);

        let jito_tip_lamports = env::var("JITO_TIP_LAMPORTS")
            .ok()
            .and_then(|v| v.parse::<u64>().ok())
            .unwrap_or(1_000); // Default: 0.000001 SOL

        // Stale transaction crank configuration (active polling fallback)
        let enable_stale_crank = env::var("ENABLE_STALE_CRANK")
            .map(|v| v == "true" || v == "1")
            .unwrap_or(true); // Enabled by default for reliability

        let crank_poll_interval_secs = env::var("CRANK_POLL_INTERVAL_SECS")
            .ok()
            .and_then(|v| v.parse::<u64>().ok())
            .unwrap_or(60); // Default: 60 seconds

        let crank_stale_after_secs = env::var("CRANK_STALE_AFTER_SECS")
            .ok()
            .and_then(|v| v.parse::<i64>().ok())
            .unwrap_or(90); // Default: 90 seconds (blockhash validity window)

        let crank_batch_size = env::var("CRANK_BATCH_SIZE")
            .ok()
            .and_then(|v| v.parse::<i64>().ok())
            .unwrap_or(20); // Default: 20 transactions per cycle

        let worker_config = WorkerConfig {
            enabled: enable_background_worker,
            enable_privacy_checks,
            ..Default::default()
        };

        Ok(Self {
            database_url,
            blockchain_rpc_url,
            signing_key,
            host,
            port,
            enable_rate_limiting,
            rate_limit_config,
            enable_background_worker,
            worker_config,
            range_api_key,
            range_api_url,
            range_risk_threshold,
            helius_webhook_secret,
            quicknode_webhook_secret,
            enable_privacy_checks,
            use_jito_bundles,
            jito_tip_lamports,
            enable_stale_crank,
            crank_poll_interval_secs,
            crank_stale_after_secs,
            crank_batch_size,
        })
    }

    fn load_signing_key() -> Result<SigningKey> {
        let key_str = env::var("ISSUER_PRIVATE_KEY").map_err(|_| {
            anyhow::anyhow!(
                "ISSUER_PRIVATE_KEY environment variable is not set.\n\
                 This is a REQUIRED configuration for production.\n\
                 Please set ISSUER_PRIVATE_KEY to a valid Base58-encoded Solana private key."
            )
        })?;

        if key_str.is_empty() {
            anyhow::bail!(
                "ISSUER_PRIVATE_KEY environment variable is empty.\n\
                 Please provide a valid Base58-encoded Solana private key."
            );
        }

        if key_str == "YOUR_BASE58_ENCODED_PRIVATE_KEY_HERE" {
            anyhow::bail!(
                "ISSUER_PRIVATE_KEY is set to the default placeholder value.\n\
                 Please replace it with your actual Base58-encoded Solana private key.\n\
                 SECURITY WARNING: Never run in production without a valid key!"
            );
        }

        info!("Loading signing key from environment");
        let secret = SecretString::from(key_str);
        signing_key_from_base58(&secret).context("Failed to parse ISSUER_PRIVATE_KEY as Base58")
    }
}

fn init_tracing() {
    let env_filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("info,tower_http=debug,sqlx=warn"));

    tracing_subscriber::registry()
        .with(env_filter)
        .with(tracing_subscriber::fmt::layer())
        .init();
}

async fn shutdown_signal() {
    let ctrl_c = async {
        signal::ctrl_c()
            .await
            .expect("Failed to install Ctrl+C handler");
    };

    #[cfg(unix)]
    let terminate = async {
        signal::unix::signal(signal::unix::SignalKind::terminate())
            .expect("Failed to install signal handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => info!("Received Ctrl+C"),
        _ = terminate => info!("Received SIGTERM"),
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    dotenv().ok();
    init_tracing();

    info!(
        "🏗️  Solana Compliance Relayer v{}",
        env!("CARGO_PKG_VERSION")
    );

    let config = Config::from_env()?;

    let public_key = bs58::encode(config.signing_key.verifying_key().as_bytes()).into_string();
    info!("🔑 Public key: {}", public_key);

    info!("📦 Initializing infrastructure...");

    // Initialize database
    let db_config = PostgresConfig::default();
    let postgres_client = PostgresClient::new(&config.database_url, db_config).await?;
    postgres_client.run_migrations().await?;
    info!("   ✓ Database connected and migrations applied");

    // Get pool reference for blocklist manager (before moving postgres_client into Arc)
    let db_pool = postgres_client.pool().clone();

    // Initialize blockchain client with optional Jito bundle submission
    let provider_type = RpcProviderType::detect(&config.blockchain_rpc_url);

    // Build submission strategy if Jito bundles are enabled and provider is QuickNode
    // Also track the tip amount for injection into transactions
    let (submission_strategy, jito_tip_for_client): (
        Option<Box<dyn solana_compliance_relayer::infra::blockchain::SubmissionStrategy>>,
        Option<u64>,
    ) = if config.use_jito_bundles {
        if matches!(provider_type, RpcProviderType::QuickNode) {
            // Read optional Jito region (e.g., "ny", "amsterdam", "frankfurt", "tokyo")
            let jito_region = env::var("JITO_REGION").ok();

            let jito_config = QuickNodeSubmissionConfig {
                rpc_url: config.blockchain_rpc_url.clone(),
                enable_jito_bundles: true,
                tip_lamports: config.jito_tip_lamports,
                max_bundle_retries: 2,
                region: jito_region.clone(),
            };
            info!(
                "   ✓ Jito bundle submission enabled (tip: {} lamports, region: {:?})",
                config.jito_tip_lamports,
                jito_region.as_deref().unwrap_or("auto")
            );
            (
                Some(Box::new(QuickNodePrivateSubmissionStrategy::new(
                    jito_config,
                ))),
                Some(config.jito_tip_lamports),
            )
        } else {
            warn!(
                "   ⚠ USE_JITO_BUNDLES=true but provider is {} (not QuickNode) - Jito disabled",
                provider_type.name()
            );
            (None, None)
        }
    } else {
        info!("   ○ Jito bundle submission disabled (standard RPC mode)");
        (None, None)
    };

    let blockchain_client = RpcBlockchainClient::with_defaults_and_submission_strategy(
        &config.blockchain_rpc_url,
        config.signing_key,
        submission_strategy,
        jito_tip_for_client,
    )?;
    info!("   ✓ Blockchain client created");

    let compliance_provider = solana_compliance_relayer::infra::RangeComplianceProvider::new(
        config.range_api_key.clone(),
        config.range_api_url.clone(),
        Some(config.range_risk_threshold),
    );
    if config.range_api_key.is_some() {
        info!("   ✓ Compliance provider created (Range Protocol API)");
        info!("   ✓ Risk threshold: {}", config.range_risk_threshold);
    } else {
        warn!("   ⚠ Compliance provider created (MOCK MODE - no RANGE_API_KEY)");
    }

    // Initialize internal blocklist manager (uses db_pool directly)
    let blocklist = BlocklistManager::new(db_pool).await?;
    info!(
        "   ✓ Blocklist manager initialized ({} entries loaded)",
        blocklist.len()
    );
    let blocklist = Arc::new(blocklist);

    // Create application state
    let app_state = AppState::with_webhook_secrets(
        Arc::new(postgres_client),
        Arc::new(blockchain_client),
        Arc::new(compliance_provider),
        config.helius_webhook_secret.clone(),
        config.quicknode_webhook_secret.clone(),
    );

    if config.helius_webhook_secret.is_some() {
        info!("   ✓ Helius webhook secret configured");
    } else {
        info!("   ○ Helius webhook secret not configured (webhook auth disabled)");
    }

    if config.quicknode_webhook_secret.is_some() {
        info!("   ✓ QuickNode webhook secret configured");
    } else {
        info!("   ○ QuickNode webhook secret not configured (webhook auth disabled)");
    }

    // Initialize privacy health check service (QuickNode only)
    let privacy_service = if config.enable_privacy_checks {
        let provider_type = RpcProviderType::detect(&config.blockchain_rpc_url);

        if matches!(provider_type, RpcProviderType::QuickNode) {
            let token_api_client =
                Arc::new(QuickNodeTokenApiClient::new(&config.blockchain_rpc_url));
            let privacy_config = PrivacyHealthCheckConfig::from_env();
            let service = Arc::new(PrivacyHealthCheckService::new(
                privacy_config,
                Some(token_api_client),
            ));
            info!("   ✓ Privacy Health Check service initialized (QuickNode)");
            Some(service)
        } else {
            info!("   ○ Privacy Health Check disabled (requires QuickNode RPC)");
            None
        }
    } else {
        info!("   ○ Privacy Health Check disabled via config");
        None
    };

    // Add privacy service and blocklist to app state
    let app_state = if let Some(ref privacy_svc) = privacy_service {
        app_state.with_privacy_service(Arc::clone(privacy_svc))
    } else {
        app_state
    };
    let app_state = app_state.with_blocklist(Arc::clone(&blocklist));

    // Initialize risk service for pre-flight compliance checks
    let range_provider_arc = Arc::new(
        solana_compliance_relayer::infra::RangeComplianceProvider::new(
            config.range_api_key.clone(),
            config.range_api_url.clone(),
            Some(config.range_risk_threshold),
        ),
    );
    let risk_service = Arc::new(RiskService::new(
        Arc::clone(&app_state.db_client),
        Arc::clone(&app_state.blockchain_client),
        range_provider_arc,
        Some(blocklist),
    ));
    let app_state = app_state.with_risk_service(risk_service);
    info!("   ✓ Risk check service initialized");

    let app_state = Arc::new(app_state);

    // Start background worker if enabled
    let worker_shutdown_tx = if config.enable_background_worker {
        let (_worker_handle, shutdown_tx) = if let Some(ref privacy_svc) = privacy_service {
            spawn_worker_with_privacy(
                Arc::clone(&app_state.service),
                config.worker_config.clone(),
                Arc::clone(privacy_svc),
            )
        } else {
            spawn_worker(Arc::clone(&app_state.service), config.worker_config.clone())
        };
        info!("   ✓ Background worker started");
        Some(shutdown_tx)
    } else {
        info!("   ○ Background worker disabled");
        None
    };

    // Start stale transaction crank (active polling fallback for webhook failures)
    let crank_shutdown_tx = if config.enable_stale_crank && config.enable_background_worker {
        let crank_config = CrankConfig {
            poll_interval: std::time::Duration::from_secs(config.crank_poll_interval_secs),
            stale_after_secs: config.crank_stale_after_secs,
            batch_size: config.crank_batch_size,
            enabled: true,
        };
        let (_crank_handle, shutdown_tx) =
            spawn_crank(Arc::clone(&app_state.service), crank_config);
        info!(
            "   ✓ Stale transaction crank started (poll: {}s, stale_after: {}s)",
            config.crank_poll_interval_secs, config.crank_stale_after_secs
        );
        Some(shutdown_tx)
    } else if !config.enable_stale_crank {
        info!("   ○ Stale transaction crank disabled");
        None
    } else {
        // Background worker disabled, so crank is also disabled
        None
    };

    // Create router
    let router = if config.enable_rate_limiting {
        info!("   ✓ Rate limiting enabled");
        create_router_with_rate_limit(app_state, config.rate_limit_config)
    } else {
        info!("   ○ Rate limiting disabled");
        create_router(app_state)
    };

    let addr = format!("{}:{}", config.host, config.port);
    let listener = tokio::net::TcpListener::bind(&addr).await?;

    info!("🚀 Server starting on http://{}", addr);
    info!("📖 Swagger UI available at http://{}/swagger-ui", addr);
    info!("📄 OpenAPI spec at http://{}/api-docs/openapi.json", addr);

    axum::serve(listener, router)
        .with_graceful_shutdown(shutdown_signal())
        .await?;

    // Signal worker and crank to shutdown
    if let Some(tx) = worker_shutdown_tx {
        let _ = tx.send(true);
    }
    if let Some(tx) = crank_shutdown_tx {
        let _ = tx.send(true);
    }

    info!("Server shutdown complete");
    Ok(())
}

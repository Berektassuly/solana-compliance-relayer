//! Application entry point.

use std::env;
use std::sync::Arc;

use anyhow::{Context, Result};
use dotenvy::dotenv;
use ed25519_dalek::SigningKey;
use rand::rngs::OsRng;
use secrecy::SecretString;
use tokio::signal;
use tracing::{info, warn};
use tracing_subscriber::{EnvFilter, layer::SubscriberExt, util::SubscriberInitExt};

use solana_compliance_relayer::api::{
    RateLimitConfig, create_router, create_router_with_rate_limit,
};
use solana_compliance_relayer::app::{AppState, WorkerConfig, spawn_worker};
use solana_compliance_relayer::infra::RpcBlockchainClient;
use solana_compliance_relayer::infra::{PostgresClient, PostgresConfig, signing_key_from_base58};

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
    /// Helius webhook secret for authentication (optional)
    helius_webhook_secret: Option<String>,
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

        let rate_limit_config = RateLimitConfig::from_env();
        let worker_config = WorkerConfig {
            enabled: enable_background_worker,
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
            helius_webhook_secret,
        })
    }

    fn load_signing_key() -> Result<SigningKey> {
        match env::var("ISSUER_PRIVATE_KEY").ok() {
            Some(key_str)
                if !key_str.is_empty() && key_str != "YOUR_BASE58_ENCODED_PRIVATE_KEY_HERE" =>
            {
                info!("Loading signing key from environment");
                let secret = SecretString::from(key_str);
                signing_key_from_base58(&secret).context("Failed to parse ISSUER_PRIVATE_KEY")
            }
            _ => {
                warn!("No valid ISSUER_PRIVATE_KEY, generating ephemeral keypair");
                Ok(SigningKey::generate(&mut OsRng))
            }
        }
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

    // Initialize blockchain client
    let blockchain_client =
        RpcBlockchainClient::with_defaults(&config.blockchain_rpc_url, config.signing_key)?;
    info!("   ✓ Blockchain client created");

    // Initialize compliance provider
    let compliance_provider = solana_compliance_relayer::infra::RangeComplianceProvider::new(
        config.range_api_key.clone(),
        config.range_api_url.clone(),
    );
    if config.range_api_key.is_some() {
        info!("   ✓ Compliance provider created (Range Protocol API)");
    } else {
        warn!("   ⚠ Compliance provider created (MOCK MODE - no RANGE_API_KEY)");
    }

    // Create application state
    let app_state = Arc::new(AppState::with_helius_secret(
        Arc::new(postgres_client),
        Arc::new(blockchain_client),
        Arc::new(compliance_provider),
        config.helius_webhook_secret.clone(),
    ));

    if config.helius_webhook_secret.is_some() {
        info!("   ✓ Helius webhook secret configured");
    } else {
        info!("   ○ Helius webhook secret not configured (webhook auth disabled)");
    }

    // Start background worker if enabled
    let worker_shutdown_tx = if config.enable_background_worker {
        let (_handle, shutdown_tx) =
            spawn_worker(Arc::clone(&app_state.service), config.worker_config);
        info!("   ✓ Background worker started");
        Some(shutdown_tx)
    } else {
        info!("   ○ Background worker disabled");
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

    // Signal worker to shutdown
    if let Some(tx) = worker_shutdown_tx {
        let _ = tx.send(true);
    }

    info!("Server shutdown complete");
    Ok(())
}

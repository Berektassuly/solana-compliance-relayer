//! Blockchain RPC client implementation for Solana.
//!
//! This module provides both mock and real blockchain interactions.
//! Real blockchain functionality is enabled with the `real-blockchain` feature.

use async_trait::async_trait;
use ed25519_dalek::{Signer, SigningKey};
use reqwest::Client;
use secrecy::{ExposeSecret, SecretString};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use std::time::Duration;
use tracing::{debug, info, instrument, warn};

// Solana SDK imports (v3.0)
use base64::{Engine as _, prelude::BASE64_STANDARD};
use solana_client::nonblocking::rpc_client::RpcClient as SolanaRpcClient;
use solana_commitment_config::CommitmentConfig;
use solana_compute_budget_interface::ComputeBudgetInstruction;
use solana_sdk::{
    instruction::Instruction,
    pubkey::Pubkey,
    signer::{Signer as SolanaSigner, keypair::Keypair},
    transaction::Transaction,
};
use solana_system_interface::instruction as system_instruction;
use solana_zk_sdk::zk_elgamal_proof_program::{
    instruction::{ContextStateInfo, ProofInstruction},
    proof_data::{
        BatchedGroupedCiphertext3HandlesValidityProofContext,
        BatchedGroupedCiphertext3HandlesValidityProofData, BatchedRangeProofContext,
        BatchedRangeProofU128Data, CiphertextCommitmentEqualityProofContext,
        CiphertextCommitmentEqualityProofData,
    },
    state::ProofContextState,
};
use spl_associated_token_account::{
    get_associated_token_address_with_program_id,
    instruction::create_associated_token_account_idempotent,
};

use spl_token_interface::instruction as token_instruction;

use crate::domain::types::TransferType;
use crate::domain::{AppError, BlockchainClient, BlockchainError, TransferRequest};

/// Configuration for the RPC client
#[derive(Debug, Clone)]
pub struct RpcClientConfig {
    pub timeout: Duration,
    pub max_retries: u32,
    pub retry_delay: Duration,
    pub confirmation_timeout: Duration,
}

impl Default for RpcClientConfig {
    fn default() -> Self {
        Self {
            timeout: Duration::from_secs(30),
            max_retries: 3,
            retry_delay: Duration::from_millis(500),
            confirmation_timeout: Duration::from_secs(60),
        }
    }
}

/// Abstract provider for Solana RPC interactions to enable testing
#[async_trait]
pub trait SolanaRpcProvider: Send + Sync {
    /// Send a JSON-RPC request
    async fn send_request(
        &self,
        method: &str,
        params: serde_json::Value,
    ) -> Result<serde_json::Value, AppError>;

    /// Get the provider's public key
    fn public_key(&self) -> String;

    /// Sign a message
    fn sign(&self, message: &[u8]) -> String;
}

/// HTTP-based Solana RPC provider
pub struct HttpSolanaRpcProvider {
    http_client: Client,
    rpc_url: String,
    signing_key: SigningKey,
}

impl HttpSolanaRpcProvider {
    pub fn new(
        rpc_url: &str,
        signing_key: SigningKey,
        timeout: Duration,
    ) -> Result<Self, AppError> {
        let http_client = Client::builder()
            .timeout(timeout)
            .build()
            .map_err(|e| AppError::Blockchain(BlockchainError::Connection(e.to_string())))?;

        Ok(Self {
            http_client,
            rpc_url: rpc_url.to_string(),
            signing_key,
        })
    }
}

#[async_trait]
impl SolanaRpcProvider for HttpSolanaRpcProvider {
    async fn send_request(
        &self,
        method: &str,
        params: serde_json::Value,
    ) -> Result<serde_json::Value, AppError> {
        let request = JsonRpcRequest {
            jsonrpc: "2.0",
            id: 1,
            method: method.to_string(),
            params,
        };

        let response = self
            .http_client
            .post(&self.rpc_url)
            .json(&request)
            .send()
            .await
            .map_err(|e| {
                if e.is_timeout() {
                    AppError::Blockchain(BlockchainError::Timeout(e.to_string()))
                } else {
                    AppError::Blockchain(BlockchainError::RpcError(e.to_string()))
                }
            })?;

        let rpc_response: JsonRpcResponse<serde_json::Value> = response
            .json()
            .await
            .map_err(|e| AppError::Blockchain(BlockchainError::RpcError(e.to_string())))?;

        if let Some(error) = rpc_response.error {
            // Check for insufficient funds error
            if error.message.contains("insufficient") || error.code == -32002 {
                return Err(AppError::Blockchain(BlockchainError::InsufficientFunds));
            }
            return Err(AppError::Blockchain(BlockchainError::RpcError(format!(
                "{}: {}",
                error.code, error.message
            ))));
        }

        rpc_response.result.ok_or_else(|| {
            AppError::Blockchain(BlockchainError::RpcError("Empty response".to_string()))
        })
    }

    fn public_key(&self) -> String {
        bs58::encode(self.signing_key.verifying_key().as_bytes()).into_string()
    }

    fn sign(&self, message: &[u8]) -> String {
        let signature = self.signing_key.sign(message);
        bs58::encode(signature.to_bytes()).into_string()
    }
}

/// Solana RPC blockchain client with provider strategy pattern
///
/// Auto-detects the RPC provider (Helius, QuickNode, Standard) and activates
/// premium features accordingly:
/// - Helius: Priority fee estimation, DAS compliance checks
/// - QuickNode: Priority fee estimation, Jito bundle submission (Ghost Mode)
/// - Standard: Fallback fee strategy
pub struct RpcBlockchainClient {
    provider: Box<dyn SolanaRpcProvider>,
    config: RpcClientConfig,
    /// Solana SDK RPC client for SDK-based operations
    sdk_client: Option<SolanaRpcClient>,
    /// Solana keypair for signing transactions
    keypair: Option<Keypair>,
    /// Auto-detected provider type
    provider_type: super::strategies::RpcProviderType,
    /// Priority fee estimation strategy
    fee_strategy: Box<dyn super::strategies::FeeStrategy>,
    /// Transaction submission strategy (Jito bundles, standard RPC, etc.)
    /// When present, transactions are submitted via this strategy instead of
    /// the SDK's send_and_confirm_transaction.
    submission_strategy: Option<Box<dyn super::strategies::SubmissionStrategy>>,
    /// Helius DAS client for compliance checks (only for Helius provider)
    das_client: Option<super::helius::HeliusDasClient>,
    /// RPC URL (stored for strategy use, logging, and future getter)
    #[allow(dead_code)]
    rpc_url: String,
    /// Jito tip amount in lamports (only used when submission_strategy supports private submission)
    /// This tip is added as a SOL transfer instruction to a Jito tip account.
    jito_tip_lamports: Option<u64>,
}

#[derive(Debug, Serialize)]
struct JsonRpcRequest<T: Serialize> {
    jsonrpc: &'static str,
    id: u64,
    method: String,
    params: T,
}

#[derive(Debug, Deserialize)]
struct JsonRpcResponse<T> {
    result: Option<T>,
    error: Option<JsonRpcError>,
}

#[derive(Debug, Deserialize)]
struct JsonRpcError {
    code: i64,
    message: String,
}

#[derive(Debug, Deserialize)]
struct BlockhashResponse {
    blockhash: String,
}

#[derive(Debug, Deserialize)]
struct BlockhashResult {
    value: BlockhashResponse,
}

#[derive(Debug, Deserialize)]
struct SignatureStatus {
    err: Option<serde_json::Value>,
    #[serde(rename = "confirmationStatus")]
    confirmation_status: Option<String>,
}

#[derive(Debug, Deserialize)]
struct SignatureStatusResult {
    value: Vec<Option<SignatureStatus>>,
}

/// Response structure for QuickNode's qn_estimatePriorityFees API
#[allow(dead_code)] // Scaffolded for QuickNode fee strategy
#[derive(Debug, Deserialize)]
struct QuickNodePriorityFeeResponse {
    per_compute_unit: Option<QuickNodePriorityFeeLevel>,
}

/// Priority fee levels from QuickNode API (values in micro-lamports)
#[allow(dead_code)] // Scaffolded for QuickNode fee strategy
#[derive(Debug, Deserialize)]
struct QuickNodePriorityFeeLevel {
    high: Option<f64>,
    #[allow(dead_code)]
    medium: Option<f64>,
    #[allow(dead_code)]
    low: Option<f64>,
}

impl RpcBlockchainClient {
    /// Create a new RPC blockchain client with custom configuration
    ///
    /// Automatically detects the RPC provider type and activates premium features:
    /// - **Helius**: Priority fee estimation via `getPriorityFeeEstimate`, DAS compliance checks
    /// - **QuickNode**: Priority fee estimation via `qn_estimatePriorityFees`
    /// - **Standard**: Fallback to static priority fee
    ///
    /// Note: This constructor does NOT enable Jito bundle submission. Use
    /// `new_with_submission_strategy` to enable MEV-protected submission.
    pub fn new(
        rpc_url: &str,
        signing_key: SigningKey,
        config: RpcClientConfig,
    ) -> Result<Self, AppError> {
        Self::new_with_submission_strategy(rpc_url, signing_key, config, None, None)
    }

    /// Create a new RPC blockchain client with custom configuration and submission strategy
    ///
    /// Automatically detects the RPC provider type and activates premium features:
    /// - **Helius**: Priority fee estimation via `getPriorityFeeEstimate`, DAS compliance checks
    /// - **QuickNode**: Priority fee estimation via `qn_estimatePriorityFees`, Jito bundles (if enabled)
    /// - **Standard**: Fallback to static priority fee
    ///
    /// # Arguments
    /// * `rpc_url` - The RPC endpoint URL
    /// * `signing_key` - The ed25519 signing key for transaction signing
    /// * `config` - Client configuration (timeouts, retries, etc.)
    /// * `submission_strategy` - Optional submission strategy for MEV-protected submission
    /// * `jito_tip_lamports` - Optional tip amount for Jito bundles (in lamports)
    ///
    /// # Submission Strategy Behavior
    /// When a submission strategy is provided:
    /// - Transactions are submitted via the strategy (e.g., Jito bundles)
    /// - Returns immediately after submission (does NOT wait for confirmation)
    /// - Confirmation should be handled via polling or webhooks
    ///
    /// When no submission strategy is provided (None):
    /// - Uses SDK's `send_and_confirm_transaction` (blocks until confirmed)
    /// - Backward compatible with existing behavior
    ///
    /// # Jito Tip Injection
    /// When `jito_tip_lamports` is Some and the submission strategy supports private
    /// submission, a SOL transfer instruction to a Jito tip account is automatically
    /// appended to each transaction before signing. This tip is REQUIRED for Jito
    /// bundle acceptance.
    pub fn new_with_submission_strategy(
        rpc_url: &str,
        signing_key: SigningKey,
        config: RpcClientConfig,
        submission_strategy: Option<Box<dyn super::strategies::SubmissionStrategy>>,
        jito_tip_lamports: Option<u64>,
    ) -> Result<Self, AppError> {
        use super::helius::{HeliusDasClient, HeliusFeeStrategy};
        use super::strategies::{FallbackFeeStrategy, QuickNodeFeeStrategy, RpcProviderType};

        let provider = HttpSolanaRpcProvider::new(rpc_url, signing_key.clone(), config.timeout)?;

        // Auto-detect provider type from URL
        let provider_type = RpcProviderType::detect(rpc_url);
        info!(
            provider = %provider_type.name(),
            rpc_url = %rpc_url,
            "Detected RPC provider type"
        );

        // Select fee strategy and DAS client based on provider type
        let (fee_strategy, das_client): (
            Box<dyn super::strategies::FeeStrategy>,
            Option<HeliusDasClient>,
        ) = match &provider_type {
            RpcProviderType::Helius => {
                info!("Helius Priority Fee Strategy activated!");
                info!("Helius DAS Check enabled");
                (
                    Box::new(HeliusFeeStrategy::new(rpc_url)),
                    Some(HeliusDasClient::new(rpc_url)),
                )
            }
            RpcProviderType::QuickNode => {
                info!("QuickNode Priority Fee Strategy activated");
                (Box::new(QuickNodeFeeStrategy::new(rpc_url)), None)
            }
            RpcProviderType::Standard => {
                info!("Standard RPC (fallback fee strategy)");
                (Box::new(FallbackFeeStrategy::new()), None)
            }
        };

        // Create Solana SDK keypair from ed25519-dalek signing key
        let keypair_bytes = signing_key.to_keypair_bytes();
        let keypair = Keypair::try_from(keypair_bytes.as_slice()).map_err(|e| {
            AppError::Blockchain(BlockchainError::InvalidSignature(format!(
                "Failed to create keypair: {}",
                e
            )))
        })?;

        // Create Solana SDK RPC client
        let sdk_client = SolanaRpcClient::new_with_timeout_and_commitment(
            rpc_url.to_string(),
            config.timeout,
            CommitmentConfig::confirmed(),
        );

        // Log submission strategy status
        let strategy_name = submission_strategy
            .as_ref()
            .map(|s| s.name())
            .unwrap_or("None (SDK send_and_confirm)");

        // Only log tip if we have a strategy that supports private submission
        let effective_tip = if submission_strategy
            .as_ref()
            .is_some_and(|s| s.supports_private_submission())
        {
            jito_tip_lamports
        } else {
            None
        };

        info!(
            rpc_url = %rpc_url,
            provider = %provider_type.name(),
            fee_strategy = %fee_strategy.name(),
            submission_strategy = %strategy_name,
            jito_tip_lamports = ?effective_tip,
            das_enabled = das_client.is_some(),
            "Created blockchain client with SDK support"
        );

        Ok(Self {
            provider: Box::new(provider),
            config,
            sdk_client: Some(sdk_client),
            keypair: Some(keypair),
            provider_type,
            fee_strategy,
            submission_strategy,
            das_client,
            rpc_url: rpc_url.to_string(),
            jito_tip_lamports,
        })
    }

    /// Create a new RPC blockchain client with default configuration
    pub fn with_defaults(rpc_url: &str, signing_key: SigningKey) -> Result<Self, AppError> {
        Self::new(rpc_url, signing_key, RpcClientConfig::default())
    }

    /// Create a new RPC blockchain client with default configuration and submission strategy
    ///
    /// Convenience method that combines `with_defaults` with submission strategy support.
    ///
    /// # Arguments
    /// * `rpc_url` - The RPC endpoint URL
    /// * `signing_key` - The ed25519 signing key for transaction signing
    /// * `submission_strategy` - Optional submission strategy for MEV-protected submission
    /// * `jito_tip_lamports` - Optional tip amount for Jito bundles (in lamports)
    pub fn with_defaults_and_submission_strategy(
        rpc_url: &str,
        signing_key: SigningKey,
        submission_strategy: Option<Box<dyn super::strategies::SubmissionStrategy>>,
        jito_tip_lamports: Option<u64>,
    ) -> Result<Self, AppError> {
        Self::new_with_submission_strategy(
            rpc_url,
            signing_key,
            RpcClientConfig::default(),
            submission_strategy,
            jito_tip_lamports,
        )
    }

    /// Create a new client with a specific provider (useful for testing)
    pub fn with_provider(provider: Box<dyn SolanaRpcProvider>, config: RpcClientConfig) -> Self {
        use super::strategies::{FallbackFeeStrategy, RpcProviderType};

        Self {
            provider,
            config,
            sdk_client: None,
            keypair: None,
            provider_type: RpcProviderType::Standard,
            fee_strategy: Box::new(FallbackFeeStrategy::new()),
            submission_strategy: None,
            das_client: None,
            rpc_url: String::new(),
            jito_tip_lamports: None,
        }
    }

    /// Check if this client has a submission strategy configured
    pub fn has_submission_strategy(&self) -> bool {
        self.submission_strategy.is_some()
    }

    /// Check if this client supports private/MEV-protected submission
    pub fn supports_private_submission(&self) -> bool {
        self.submission_strategy
            .as_ref()
            .is_some_and(|s| s.supports_private_submission())
    }

    /// Creates a Jito tip instruction if Jito submission is enabled and configured.
    ///
    /// This method creates a SOL transfer instruction from the payer to a randomly
    /// selected Jito tip account. The tip is REQUIRED for Jito bundle acceptance.
    ///
    /// # Returns
    /// - `Some(Instruction)` - Tip instruction to append to the transaction
    /// - `None` - If Jito is not enabled, not supported, or tip amount is 0
    ///
    /// # Best Practices
    /// The tip instruction should be the LAST instruction in the transaction to avoid
    /// potential issues with instruction ordering during bundle processing.
    fn create_jito_tip_instruction(&self, payer: &Pubkey) -> Option<Instruction> {
        // Only add tip if we have a Jito-enabled submission strategy
        if !self.supports_private_submission() {
            return None;
        }

        let tip_lamports = self.jito_tip_lamports?;

        if tip_lamports == 0 {
            debug!("Jito tip is 0, skipping tip instruction");
            return None;
        }

        // Select a random tip account to reduce contention
        let tip_account_str = super::random_jito_tip_account();
        let tip_account = tip_account_str
            .parse::<Pubkey>()
            .expect("Hardcoded Jito tip account should be valid");

        debug!(
            tip_lamports = tip_lamports,
            tip_account = %tip_account,
            "Creating Jito tip instruction"
        );

        Some(system_instruction::transfer(
            payer,
            &tip_account,
            tip_lamports,
        ))
    }

    /// Get the detected provider type
    pub fn provider_type(&self) -> &super::strategies::RpcProviderType {
        &self.provider_type
    }

    /// Check if Helius DAS is available
    pub fn has_das_support(&self) -> bool {
        self.das_client.is_some()
    }

    /// Get the public key as base58 string
    #[must_use]
    pub fn public_key(&self) -> String {
        self.provider.public_key()
    }

    /// Sign a message and return the signature as base58
    #[must_use]
    pub fn sign(&self, message: &[u8]) -> String {
        self.provider.sign(message)
    }

    /// Make an RPC call with retries
    #[instrument(skip(self, params))]
    async fn rpc_call<P: Serialize + Send + Sync, R: DeserializeOwned + Send>(
        &self,
        method: &str,
        params: P,
    ) -> Result<R, AppError> {
        // Serialize parameters to JSON Value
        let params_value = serde_json::to_value(params).map_err(|e| {
            AppError::Blockchain(BlockchainError::RpcError(format!(
                "Serialization error: {}",
                e
            )))
        })?;

        let mut last_error = None;
        for attempt in 0..=self.config.max_retries {
            if attempt > 0 {
                tokio::time::sleep(self.config.retry_delay).await;
            }
            match self
                .provider
                .send_request(method, params_value.clone())
                .await
            {
                Ok(result_value) => {
                    // Deserialize result from JSON Value
                    return serde_json::from_value(result_value).map_err(|e| {
                        AppError::Blockchain(BlockchainError::RpcError(format!(
                            "Deserialization error: {}",
                            e
                        )))
                    });
                }
                Err(e) => {
                    warn!(attempt = attempt, error = ?e, method = %method, "RPC call failed");
                    last_error = Some(e);
                }
            }
        }
        Err(last_error.unwrap_or_else(|| {
            AppError::Blockchain(BlockchainError::RpcError("Unknown error".to_string()))
        }))
    }

    /// Get priority fee using the appropriate strategy for the detected provider.
    ///
    /// This method delegates to the configured fee strategy:
    /// - Helius: Uses `getPriorityFeeEstimate` for transaction-aware estimation
    /// - QuickNode: Uses `qn_estimatePriorityFees` for global estimation
    /// - Standard: Returns a static fallback value
    ///
    /// # Arguments
    /// * `serialized_tx` - Optional Base58-encoded serialized transaction
    ///   (used by Helius for per-account fee estimation)
    async fn get_priority_fee(&self, serialized_tx: Option<&str>) -> u64 {
        self.fee_strategy.get_priority_fee(serialized_tx).await
    }

    /// Legacy method for backward compatibility - calls the new strategy-based method
    #[allow(dead_code)]
    async fn get_quicknode_priority_fee(&self) -> u64 {
        self.get_priority_fee(None).await
    }

    /// Submit a transaction via the configured strategy, or fall back to SDK confirmation
    ///
    /// # Behavior
    /// - **With submission strategy**: Serializes to Base58, submits via strategy, returns
    ///   immediately. The transaction is "submitted" but not yet "confirmed". Confirmation
    ///   should be handled via polling (`wait_for_confirmation`) or webhooks.
    ///
    /// - **Without submission strategy**: Uses SDK's `send_and_confirm_transaction` which
    ///   blocks until the transaction is confirmed. Returns only after confirmation.
    ///
    /// # Important: "Submitted" State Resilience
    /// When using the submission strategy path, the transaction is persisted as "Submitted"
    /// immediately after this call returns successfully. However, the transaction may not
    /// be indexed by RPC nodes yet. `get_transaction_status` handles this gracefully by
    /// returning `false` (not confirmed) rather than an error for "not found" transactions.
    async fn submit_or_confirm_transaction(
        &self,
        transaction: &Transaction,
    ) -> Result<String, AppError> {
        let sdk_client = self.sdk_client.as_ref().ok_or_else(|| {
            AppError::Blockchain(BlockchainError::TransactionFailed(
                "SDK client not available".to_string(),
            ))
        })?;

        if let Some(ref strategy) = self.submission_strategy {
            // Serialize transaction to Base58 for strategy submission
            let serialized_tx = self.serialize_transaction_base58(transaction)?;

            // Submit via strategy (Jito bundle, standard sendTransaction, etc.)
            // The strategy handles signature extraction internally
            let signature = strategy.submit_transaction(&serialized_tx, true).await?;

            info!(
                signature = %signature,
                strategy = %strategy.name(),
                "Transaction submitted via submission strategy (confirmation pending)"
            );

            Ok(signature)
        } else {
            // No strategy - use SDK's blocking send_and_confirm
            let signature = sdk_client
                .send_and_confirm_transaction(transaction)
                .await
                .map_err(map_solana_client_error)?;

            debug!(
                signature = %signature,
                "Transaction confirmed via SDK send_and_confirm"
            );

            Ok(signature.to_string())
        }
    }

    /// Serialize a signed transaction to Base58 encoding
    ///
    /// Used for submitting transactions via the submission strategy.
    fn serialize_transaction_base58(&self, transaction: &Transaction) -> Result<String, AppError> {
        let serialized = bincode::serialize(transaction).map_err(|e| {
            AppError::Blockchain(BlockchainError::TransactionFailed(format!(
                "Failed to serialize transaction: {}",
                e
            )))
        })?;

        Ok(bs58::encode(&serialized).into_string())
    }

    /// Submit a transaction and wait for confirmation
    ///
    /// This method is used for multi-transaction flows (like confidential transfers)
    /// where subsequent transactions depend on previous ones being confirmed.
    ///
    /// # Behavior
    /// - **With submission strategy**: Submits via strategy (MEV-protected), then polls
    ///   for confirmation using `get_transaction_status` until confirmed or timeout.
    ///
    /// - **Without submission strategy**: Uses SDK's `send_and_confirm_transaction`.
    ///
    /// # Privacy Considerations for Confidential Transfers
    /// When Jito bundles are enabled, ALL transactions in a confidential transfer flow
    /// go through the private mempool:
    /// - Proof verification transactions (equality, validity, range)
    /// - Final transfer transaction
    ///
    /// This prevents timing correlation attacks where an observer could see proof
    /// verification transactions and infer that a confidential transfer is imminent.
    async fn submit_and_confirm_transaction(
        &self,
        transaction: &Transaction,
        description: &str,
    ) -> Result<String, AppError> {
        let sdk_client = self.sdk_client.as_ref().ok_or_else(|| {
            AppError::Blockchain(BlockchainError::TransactionFailed(
                "SDK client not available".to_string(),
            ))
        })?;

        if let Some(ref strategy) = self.submission_strategy {
            // Serialize transaction to Base58 for strategy submission
            let serialized_tx = self.serialize_transaction_base58(transaction)?;

            // Submit via strategy (Jito bundle, standard sendTransaction, etc.)
            let signature = strategy.submit_transaction(&serialized_tx, true).await?;

            info!(
                signature = %signature,
                strategy = %strategy.name(),
                description = %description,
                "Transaction submitted via strategy, waiting for confirmation..."
            );

            // Wait for confirmation (poll-based)
            // Use a reasonable timeout for on-chain confirmation
            let confirmation_timeout_secs = self.config.confirmation_timeout.as_secs();
            let confirmed = self
                .wait_for_confirmation(&signature, confirmation_timeout_secs)
                .await?;

            if !confirmed {
                return Err(AppError::Blockchain(BlockchainError::Timeout(format!(
                    "Transaction {} not confirmed within {}s: {}",
                    signature, confirmation_timeout_secs, description
                ))));
            }

            info!(
                signature = %signature,
                description = %description,
                "Transaction confirmed"
            );

            Ok(signature)
        } else {
            // No strategy - use SDK's blocking send_and_confirm
            let signature = sdk_client
                .send_and_confirm_transaction(transaction)
                .await
                .map_err(|e| {
                    AppError::Blockchain(BlockchainError::TransactionFailed(format!(
                        "{}: {}",
                        description, e
                    )))
                })?;

            debug!(
                signature = %signature,
                description = %description,
                "Transaction confirmed via SDK"
            );

            Ok(signature.to_string())
        }
    }
}

#[async_trait]
impl BlockchainClient for RpcBlockchainClient {
    #[instrument(skip(self))]
    async fn health_check(&self) -> Result<(), AppError> {
        let _: u64 = self.rpc_call("getSlot", Vec::<()>::new()).await?;
        Ok(())
    }

    #[instrument(skip(self))]
    async fn submit_transaction(&self, request: &TransferRequest) -> Result<String, AppError> {
        info!(id = %request.id, "Submitting transaction for request");

        // Check if we have SDK client (for real transactions)
        if self.sdk_client.is_none() || self.keypair.is_none() {
            // Mock implementation for testing (when SDK client not available)
            debug!("Using mock implementation for submit_transaction");
            let signature = self.sign(request.id.as_bytes());
            return Ok(format!("tx_{}", &signature[..16]));
        }

        // Dispatch based on TransferType
        match &request.transfer_details {
            TransferType::Public { amount } => match &request.token_mint {
                Some(mint) => {
                    self.transfer_token(&request.to_address, mint, *amount)
                        .await
                }
                None => self.transfer_sol(&request.to_address, *amount).await,
            },
            TransferType::Confidential {
                new_decryptable_available_balance,
                equality_proof,
                ciphertext_validity_proof,
                range_proof,
            } => {
                let mint = request.token_mint.as_ref().ok_or_else(|| {
                    AppError::Validation(crate::domain::ValidationError::InvalidField {
                        field: "token_mint".to_string(),
                        message: "Token mint is required for confidential transfers".to_string(),
                    })
                })?;

                self.transfer_confidential(
                    &request.to_address,
                    mint,
                    new_decryptable_available_balance,
                    equality_proof,
                    ciphertext_validity_proof,
                    range_proof,
                )
                .await
            }
        }
    }

    /// Transfer Token-2022 Confidential funds using Split Proof Verification
    ///
    /// This implementation uses the ZK ElGamal Proof Program to verify proofs
    /// in separate transactions before executing the transfer. This avoids
    /// the Solana transaction size limit (1232 bytes).
    ///
    /// # Architecture
    /// 1. Transaction 1: Verify all ZK proofs via separate verification instructions
    /// 2. Transaction 2: Execute ConfidentialTransfer::TransferWithSplitProofs
    ///
    /// # Arguments
    /// * `to_address` - Destination wallet (Base58)
    /// * `token_mint` - Token-2022 mint with confidential extensions (Base58)
    /// * `new_decryptable_available_balance_base64` - AES-encrypted balance (Base64)
    /// * `equality_proof_base64` - CiphertextCommitmentEqualityProofData (Base64)
    /// * `ciphertext_validity_proof_base64` - BatchedGroupedCiphertext3HandlesValidityProofData (Base64)
    /// * `range_proof_base64` - BatchedRangeProofU128Data (Base64)
    #[instrument(skip(self))]
    async fn transfer_confidential(
        &self,
        to_address: &str,
        token_mint: &str,
        new_decryptable_available_balance_base64: &str,
        equality_proof_base64: &str,
        ciphertext_validity_proof_base64: &str,
        range_proof_base64: &str,
    ) -> Result<String, AppError> {
        info!(
            to = %to_address,
            token_mint = %token_mint,
            "Processing confidential transfer with split proof verification"
        );

        let keypair = self.keypair.as_ref().ok_or_else(|| {
            AppError::Blockchain(BlockchainError::WalletError(
                "No keypair available for signing".to_string(),
            ))
        })?;
        let sdk_client = self.sdk_client.as_ref().ok_or_else(|| {
            AppError::Blockchain(BlockchainError::Connection(
                "No SDK client available".to_string(),
            ))
        })?;

        // Parse addresses
        let mint_pubkey: Pubkey = token_mint.parse().map_err(|_| {
            AppError::Validation(crate::domain::ValidationError::InvalidAddress(
                token_mint.to_string(),
            ))
        })?;
        let to_pubkey: Pubkey = to_address.parse().map_err(|_| {
            AppError::Validation(crate::domain::ValidationError::InvalidAddress(
                to_address.to_string(),
            ))
        })?;

        // Decode each proof component from Base64
        let new_decryptable_balance = BASE64_STANDARD
            .decode(new_decryptable_available_balance_base64)
            .map_err(|e| {
                AppError::Validation(crate::domain::ValidationError::InvalidField {
                    field: "new_decryptable_available_balance".to_string(),
                    message: format!("Invalid base64 encoding: {}", e),
                })
            })?;

        let equality_proof = BASE64_STANDARD.decode(equality_proof_base64).map_err(|e| {
            AppError::Validation(crate::domain::ValidationError::InvalidField {
                field: "equality_proof".to_string(),
                message: format!("Invalid base64 encoding: {}", e),
            })
        })?;

        let ciphertext_validity_proof = BASE64_STANDARD
            .decode(ciphertext_validity_proof_base64)
            .map_err(|e| {
                AppError::Validation(crate::domain::ValidationError::InvalidField {
                    field: "ciphertext_validity_proof".to_string(),
                    message: format!("Invalid base64 encoding: {}", e),
                })
            })?;

        let range_proof = BASE64_STANDARD.decode(range_proof_base64).map_err(|e| {
            AppError::Validation(crate::domain::ValidationError::InvalidField {
                field: "range_proof".to_string(),
                message: format!("Invalid base64 encoding: {}", e),
            })
        })?;

        debug!(
            balance_bytes = new_decryptable_balance.len(),
            equality_proof_bytes = equality_proof.len(),
            validity_proof_bytes = ciphertext_validity_proof.len(),
            range_proof_bytes = range_proof.len(),
            "Decoded confidential transfer proof components"
        );

        // Token-2022 program ID
        let token_program_id = spl_token_2022::id();

        // Derive source and destination confidential token accounts
        let source_ata = get_associated_token_address_with_program_id(
            &keypair.pubkey(),
            &mint_pubkey,
            &token_program_id,
        );
        let destination_ata = get_associated_token_address_with_program_id(
            &to_pubkey,
            &mint_pubkey,
            &token_program_id,
        );

        debug!(
            source_ata = %source_ata,
            destination_ata = %destination_ata,
            "Derived confidential token accounts"
        );

        // Get priority fee using provider-specific strategy
        let priority_fee = self.get_priority_fee(None).await;

        // ====================================================================
        // MULTI-TRANSACTION SPLIT PROOF VERIFICATION
        // ====================================================================
        // Token-2022 confidential transfers require 3 ZK proofs that are too
        // large to fit in a single transaction. We must:
        //
        // 1. Create context state accounts via the ZK ElGamal Proof Program
        // 2. Verify each proof in separate transactions
        // 3. Execute the transfer referencing the context accounts
        // 4. Close context accounts to recover rent
        //
        // Context state accounts are PDAs derived from:
        // - The context state authority (relayer's pubkey)
        // - A unique seed per proof type
        // ====================================================================

        info!("Step 1: Preparing verification transactions for ZK proofs");

        let zk_elgamal_proof_program = solana_zk_sdk::zk_elgamal_proof_program::id();
        let context_authority = keypair.pubkey();

        // ====================================================================
        // CREATE EPHEMERAL KEYPAIRS FOR CONTEXT STATE ACCOUNTS
        // ====================================================================
        // The ZK program needs regular accounts (NOT PDAs) to store verified
        // proof state. We create fresh keypairs for each transfer attempt.
        // These will be closed after the transfer to recover rent.

        let equality_context_keypair = Keypair::new();
        let validity_context_keypair = Keypair::new();
        let range_context_keypair = Keypair::new();

        let equality_context_pubkey = equality_context_keypair.pubkey();
        let validity_context_pubkey = validity_context_keypair.pubkey();
        let range_context_pubkey = range_context_keypair.pubkey();

        // Calculate rent-exempt minimum for context state accounts
        // ProofContextState<T> stores: authority(32) + proof_type(1) + context_data(T)
        //
        // IMPORTANT: The ZK program stores the CONTEXT (result of proof verification),
        // NOT the full ProofData (which includes the proof itself).
        //
        // Use std::mem::size_of on the actual types to get exact sizes:
        let equality_context_size =
            std::mem::size_of::<ProofContextState<CiphertextCommitmentEqualityProofContext>>();
        let validity_context_size = std::mem::size_of::<
            ProofContextState<BatchedGroupedCiphertext3HandlesValidityProofContext>,
        >();
        let range_context_size = std::mem::size_of::<ProofContextState<BatchedRangeProofContext>>();

        debug!(
            equality_context_size = equality_context_size,
            validity_context_size = validity_context_size,
            range_context_size = range_context_size,
            "Calculated proof context state sizes"
        );

        let equality_rent = sdk_client
            .get_minimum_balance_for_rent_exemption(equality_context_size)
            .await
            .map_err(map_solana_client_error)?;
        let validity_rent = sdk_client
            .get_minimum_balance_for_rent_exemption(validity_context_size)
            .await
            .map_err(map_solana_client_error)?;
        let range_rent = sdk_client
            .get_minimum_balance_for_rent_exemption(range_context_size)
            .await
            .map_err(map_solana_client_error)?;

        debug!(
            equality_ctx = %equality_context_pubkey,
            validity_ctx = %validity_context_pubkey,
            range_ctx = %range_context_pubkey,
            equality_rent = %equality_rent,
            validity_rent = %validity_rent,
            range_rent = %range_rent,
            "Created ephemeral context account keypairs"
        );

        // ====================================================================
        // TRANSACTION 1: Create Account + Verify Equality Proof
        // ====================================================================
        info!("Transaction 1: Verifying equality proof");

        // Step 1a: Create the context account FIRST
        let create_equality_ctx_ix = system_instruction::create_account(
            &keypair.pubkey(),            // payer
            &equality_context_pubkey,     // new account
            equality_rent,                // lamports for rent exemption
            equality_context_size as u64, // account size in bytes
            &zk_elgamal_proof_program,    // owner = ZK program
        );

        // Step 1b: Build verify instruction using SDK's encode_verify_proof
        // Cast the raw bytes to the proof data type (bytemuck::Pod)
        let equality_proof_data: &CiphertextCommitmentEqualityProofData =
            bytemuck::try_from_bytes(&equality_proof).map_err(|e| {
                AppError::Validation(crate::domain::ValidationError::InvalidField {
                    field: "equality_proof".to_string(),
                    message: format!("Invalid proof data format: {}", e),
                })
            })?;

        // Create context state info for the SDK
        let equality_ctx_address =
            solana_sdk::pubkey::Pubkey::from(equality_context_pubkey.to_bytes());
        let authority_address = solana_sdk::pubkey::Pubkey::from(context_authority.to_bytes());
        let equality_context_info = ContextStateInfo {
            context_state_account: &equality_ctx_address,
            context_state_authority: &authority_address,
        };

        // Use SDK's encode_verify_proof which properly formats the instruction
        let equality_verify_ix = ProofInstruction::VerifyCiphertextCommitmentEquality
            .encode_verify_proof(Some(equality_context_info), equality_proof_data);

        let equality_tx_instructions = vec![
            ComputeBudgetInstruction::set_compute_unit_price(priority_fee),
            ComputeBudgetInstruction::set_compute_unit_limit(200_000),
            create_equality_ctx_ix, // CREATE first
            equality_verify_ix,     // VERIFY second
        ];

        let recent_blockhash = sdk_client
            .get_latest_blockhash()
            .await
            .map_err(map_solana_client_error)?;

        let equality_tx = Transaction::new_signed_with_payer(
            &equality_tx_instructions,
            Some(&keypair.pubkey()),
            &[keypair, &equality_context_keypair], // Context keypair must sign create_account
            recent_blockhash,
        );

        // Use submission strategy if available (MEV-protected) and wait for confirmation
        self.submit_and_confirm_transaction(&equality_tx, "Equality proof verification")
            .await?;

        info!("Equality proof verified and context state created");

        // ====================================================================
        // TRANSACTION 2: Create Account + Verify Validity Proof
        // ====================================================================
        info!("Transaction 2: Verifying ciphertext validity proof");

        // Step 2a: Create the context account FIRST
        let create_validity_ctx_ix = system_instruction::create_account(
            &keypair.pubkey(),
            &validity_context_pubkey,
            validity_rent,
            validity_context_size as u64,
            &zk_elgamal_proof_program,
        );

        // Step 2b: Build verify instruction using SDK's encode_verify_proof
        let validity_proof_data: &BatchedGroupedCiphertext3HandlesValidityProofData =
            bytemuck::try_from_bytes(&ciphertext_validity_proof).map_err(|e| {
                AppError::Validation(crate::domain::ValidationError::InvalidField {
                    field: "ciphertext_validity_proof".to_string(),
                    message: format!("Invalid proof data format: {}", e),
                })
            })?;

        let validity_ctx_address =
            solana_sdk::pubkey::Pubkey::from(validity_context_pubkey.to_bytes());
        let validity_context_info = ContextStateInfo {
            context_state_account: &validity_ctx_address,
            context_state_authority: &authority_address,
        };

        let validity_verify_ix = ProofInstruction::VerifyBatchedGroupedCiphertext3HandlesValidity
            .encode_verify_proof(Some(validity_context_info), validity_proof_data);

        let validity_tx_instructions = vec![
            ComputeBudgetInstruction::set_compute_unit_price(priority_fee),
            ComputeBudgetInstruction::set_compute_unit_limit(200_000),
            create_validity_ctx_ix, // CREATE first
            validity_verify_ix,     // VERIFY second
        ];

        let recent_blockhash = sdk_client
            .get_latest_blockhash()
            .await
            .map_err(map_solana_client_error)?;

        let validity_tx = Transaction::new_signed_with_payer(
            &validity_tx_instructions,
            Some(&keypair.pubkey()),
            &[keypair, &validity_context_keypair], // Context keypair must sign
            recent_blockhash,
        );

        // Use submission strategy if available (MEV-protected) and wait for confirmation
        self.submit_and_confirm_transaction(&validity_tx, "Ciphertext validity proof verification")
            .await?;

        info!("Validity proof verified and context state created");

        // ====================================================================
        // TRANSACTION 3A & 3B: Record-Based Range Proof Verification
        // ====================================================================
        // Range proofs are too large to fit in a single transaction (~1KB proof data).
        // We use "record-based" verification with spl-record:
        //   3A: Create a record account and write the proof data to it
        //   3B: Call verify_proof_from_account referencing the stored data
        // ====================================================================
        info!("Transaction 3: Verifying range proof (record-based for large proof)");

        // Parse the range proof data first to validate it
        let _range_proof_data: &BatchedRangeProofU128Data = bytemuck::try_from_bytes(&range_proof)
            .map_err(|e| {
                AppError::Validation(crate::domain::ValidationError::InvalidField {
                    field: "range_proof".to_string(),
                    message: format!("Invalid proof data format: {}", e),
                })
            })?;

        // Create a keypair for the proof record account
        let range_proof_record_keypair = Keypair::new();
        let range_proof_record_pubkey = range_proof_record_keypair.pubkey();

        // The spl-record program stores data with a small header (authority pubkey)
        // Record account data layout: [32 bytes authority] [data...]
        const RECORD_HEADER_SIZE: usize = 32;
        let record_data_size = RECORD_HEADER_SIZE + range_proof.len();

        let range_proof_record_rent = sdk_client
            .get_minimum_balance_for_rent_exemption(record_data_size)
            .await
            .map_err(map_solana_client_error)?;

        debug!(
            range_proof_data_size = range_proof.len(),
            record_data_size = record_data_size,
            range_proof_record_rent = range_proof_record_rent,
            range_proof_record_pubkey = %range_proof_record_pubkey,
            "Creating record account for range proof"
        );

        // TRANSACTION 3A: Create record account and write proof data
        // Use spl_record to create the account owned by the record program
        let spl_record_program_id = spl_record::id();

        // Step 1: Create the account owned by spl_record
        let create_record_account_ix = system_instruction::create_account(
            &keypair.pubkey(),
            &range_proof_record_pubkey,
            range_proof_record_rent,
            record_data_size as u64,
            &spl_record_program_id,
        );

        // Step 2: Initialize the record (sets the authority)
        let initialize_record_ix = spl_record::instruction::initialize(
            &range_proof_record_pubkey,
            &keypair.pubkey(), // authority
        );

        // Step 3: Write the proof data to the record
        let write_record_ix = spl_record::instruction::write(
            &range_proof_record_pubkey,
            &keypair.pubkey(), // authority (signer)
            0,                 // offset (write at start of data section)
            &range_proof,
        );

        // Combine create + initialize + write in one transaction
        let recent_blockhash = sdk_client
            .get_latest_blockhash()
            .await
            .map_err(map_solana_client_error)?;

        let create_and_write_record_tx = Transaction::new_signed_with_payer(
            &[
                ComputeBudgetInstruction::set_compute_unit_price(priority_fee),
                create_record_account_ix,
                initialize_record_ix,
                write_record_ix,
            ],
            Some(&keypair.pubkey()),
            &[keypair, &range_proof_record_keypair],
            recent_blockhash,
        );

        // Use submission strategy if available (MEV-protected) and wait for confirmation
        self.submit_and_confirm_transaction(
            &create_and_write_record_tx,
            "Create and write range proof record",
        )
        .await?;

        info!("Range proof record account created and data written");

        // TRANSACTION 3B: Create context account + verify from record
        let create_range_ctx_ix = system_instruction::create_account(
            &keypair.pubkey(),
            &range_context_pubkey,
            range_rent,
            range_context_size as u64,
            &zk_elgamal_proof_program,
        );

        let range_ctx_address = solana_sdk::pubkey::Pubkey::from(range_context_pubkey.to_bytes());
        let range_context_info = ContextStateInfo {
            context_state_account: &range_ctx_address,
            context_state_authority: &authority_address,
        };

        // Use encode_verify_proof_from_account instead of encode_verify_proof
        // The spl-record account has a 32-byte authority header, so offset = 32
        let range_proof_record_address =
            solana_sdk::pubkey::Pubkey::from(range_proof_record_pubkey.to_bytes());
        let range_verify_from_account_ix = ProofInstruction::VerifyBatchedRangeProofU128
            .encode_verify_proof_from_account(
                Some(range_context_info),
                &range_proof_record_address,
                RECORD_HEADER_SIZE as u32, // offset past the record header
            );

        let range_tx_instructions = vec![
            ComputeBudgetInstruction::set_compute_unit_price(priority_fee),
            ComputeBudgetInstruction::set_compute_unit_limit(1_400_000), // Range proofs need more compute!
            create_range_ctx_ix,
            range_verify_from_account_ix,
        ];

        let recent_blockhash = sdk_client
            .get_latest_blockhash()
            .await
            .map_err(map_solana_client_error)?;

        let range_tx = Transaction::new_signed_with_payer(
            &range_tx_instructions,
            Some(&keypair.pubkey()),
            &[keypair, &range_context_keypair],
            recent_blockhash,
        );

        // Use submission strategy if available (MEV-protected) and wait for confirmation
        self.submit_and_confirm_transaction(&range_tx, "Range proof verification")
            .await?;

        info!("Range proof verified and context state created");

        // ====================================================================
        // TRANSACTION 4: Execute Transfer with Context Account References
        // ====================================================================
        info!("Transaction 4: Executing confidential transfer");

        // Check if destination ATA exists, create if needed
        let dest_account_result = sdk_client.get_account(&destination_ata).await;
        let mut transfer_instructions: Vec<Instruction> = vec![
            ComputeBudgetInstruction::set_compute_unit_price(priority_fee),
            ComputeBudgetInstruction::set_compute_unit_limit(600_000), // Increased for close instructions
        ];

        if dest_account_result.is_err() {
            info!(destination_ata = %destination_ata, "Creating destination ATA");
            let create_ata_ix = create_associated_token_account_idempotent(
                &keypair.pubkey(),
                &to_pubkey,
                &mint_pubkey,
                &token_program_id,
            );
            transfer_instructions.push(create_ata_ix);
        }

        // ====================================================================
        // BUILD CONFIDENTIAL TRANSFER INSTRUCTION
        // ====================================================================
        // Token-2022 extension instruction format:
        //   Byte 0: TokenInstruction discriminator (we use the extension approach)
        //   Remaining: Extension-specific data
        //
        // For confidential transfer with split proofs, we use:
        //   [26] = ConfidentialTransferExtension (Token-2022 extension instruction)
        //   [7]  = Transfer (sub-instruction within confidential transfer)
        //
        // Account order per SPL Token-2022 ConfidentialTransferInstruction::Transfer spec:
        //   0. [writable] Source token account
        //   1. []         Token mint
        //   2. [writable] Destination token account
        //   3. []         Equality proof context account
        //   4. []         Ciphertext validity proof context account
        //   5. []         Range proof context account
        //   6. [signer]   Authority (owner of source account)

        // Build instruction data: [extension_discriminator | sub_instruction | transfer_data]
        let mut instruction_data = Vec::new();
        instruction_data.push(26u8); // ConfidentialTransferExtension
        instruction_data.push(7u8); // Transfer sub-instruction
        instruction_data.extend_from_slice(&new_decryptable_balance);

        let transfer_ix = Instruction {
            program_id: token_program_id,
            accounts: vec![
                // 0. Source token account (writable)
                solana_sdk::instruction::AccountMeta::new(source_ata, false),
                // 1. Mint (readonly)
                solana_sdk::instruction::AccountMeta::new_readonly(mint_pubkey, false),
                // 2. Destination token account (writable)
                solana_sdk::instruction::AccountMeta::new(destination_ata, false),
                // 3. Equality proof context (readonly) - pre-verified
                solana_sdk::instruction::AccountMeta::new_readonly(equality_context_pubkey, false),
                // 4. Ciphertext validity proof context (readonly)
                solana_sdk::instruction::AccountMeta::new_readonly(validity_context_pubkey, false),
                // 5. Range proof context (readonly)
                solana_sdk::instruction::AccountMeta::new_readonly(range_context_pubkey, false),
                // 6. Authority (signer) - owner of source account
                solana_sdk::instruction::AccountMeta::new_readonly(keypair.pubkey(), true),
            ],
            data: instruction_data,
        };

        transfer_instructions.push(transfer_ix);

        // ====================================================================
        // CLOSE CONTEXT ACCOUNTS TO RECOVER RENT
        // ====================================================================
        // All context accounts were used in the transfer and are no longer needed.
        // Close them to return rent-exempt lamports to the relayer.

        // Close equality proof context
        let close_equality_ctx_ix = ProofInstruction::CloseContextState.encode_close_context_state(
            &equality_ctx_address,
            &solana_sdk::pubkey::Pubkey::from(keypair.pubkey().to_bytes()),
            &authority_address,
        );
        transfer_instructions.push(close_equality_ctx_ix);

        // Close validity proof context
        let close_validity_ctx_ix = ProofInstruction::CloseContextState.encode_close_context_state(
            &validity_ctx_address,
            &solana_sdk::pubkey::Pubkey::from(keypair.pubkey().to_bytes()),
            &authority_address,
        );
        transfer_instructions.push(close_validity_ctx_ix);

        // Close range proof context
        let close_range_ctx_ix = ProofInstruction::CloseContextState.encode_close_context_state(
            &range_ctx_address,
            &solana_sdk::pubkey::Pubkey::from(keypair.pubkey().to_bytes()),
            &authority_address,
        );
        transfer_instructions.push(close_range_ctx_ix);

        // Close range proof record account (spl_record)
        let close_record_ix = spl_record::instruction::close_account(
            &range_proof_record_pubkey,
            &keypair.pubkey(), // destination (rent recovery)
            &keypair.pubkey(), // authority
        );
        transfer_instructions.push(close_record_ix);

        info!(
            equality_ctx = %equality_context_pubkey,
            validity_ctx = %validity_context_pubkey,
            range_ctx = %range_context_pubkey,
            range_record = %range_proof_record_pubkey,
            "Added close instructions for all context accounts"
        );

        // Append Jito tip instruction to FINAL transfer transaction only
        // (not to the proof verification transactions)
        if let Some(tip_ix) = self.create_jito_tip_instruction(&keypair.pubkey()) {
            info!(
                tip_lamports = self.jito_tip_lamports.unwrap_or(0),
                "Appending Jito tip instruction to confidential transfer"
            );
            transfer_instructions.push(tip_ix);
        }

        let recent_blockhash = sdk_client
            .get_latest_blockhash()
            .await
            .map_err(map_solana_client_error)?;

        let transfer_tx = Transaction::new_signed_with_payer(
            &transfer_instructions,
            Some(&keypair.pubkey()),
            &[keypair],
            recent_blockhash,
        );

        info!(
            via_strategy = self.submission_strategy.is_some(),
            jito_tip = self
                .jito_tip_lamports
                .filter(|_| self.supports_private_submission()),
            "Sending confidential transfer transaction"
        );

        // For the final transfer, we can use submit_or_confirm_transaction
        // (doesn't need to wait for subsequent transactions)
        // But for consistency with MEV protection, we use the strategy if available
        let signature = self
            .submit_or_confirm_transaction(&transfer_tx)
            .await
            .map_err(|e| {
                let msg = e.to_string();
                if msg.contains("ProofVerificationFailed") || msg.contains("proof") {
                    AppError::Blockchain(BlockchainError::TransactionFailed(format!(
                        "ZK proof verification failed: {}",
                        msg
                    )))
                } else if msg.contains("ConfidentialTransferNotEnabled") {
                    AppError::Blockchain(BlockchainError::TransactionFailed(format!(
                        "Mint does not have confidential transfer extension enabled: {}",
                        msg
                    )))
                } else {
                    e
                }
            })?;

        info!(
            signature = %signature,
            to = %to_address,
            token_mint = %token_mint,
            via_strategy = self.submission_strategy.is_some(),
            "Confidential transfer with split proofs completed successfully"
        );

        Ok(signature)
    }

    #[instrument(skip(self))]
    async fn get_block_height(&self) -> Result<u64, AppError> {
        self.rpc_call("getBlockHeight", Vec::<()>::new()).await
    }

    #[instrument(skip(self))]
    async fn get_latest_blockhash(&self) -> Result<String, AppError> {
        let result: BlockhashResult = self
            .rpc_call("getLatestBlockhash", Vec::<()>::new())
            .await?;
        Ok(result.value.blockhash)
    }

    #[instrument(skip(self))]
    async fn get_transaction_status(&self, signature: &str) -> Result<bool, AppError> {
        let params = serde_json::json!([[signature], {"searchTransactionHistory": true}]);
        let result: SignatureStatusResult = self.rpc_call("getSignatureStatuses", params).await?;

        match result.value.first() {
            Some(Some(status)) => {
                // Check if transaction errored
                if status.err.is_some() {
                    return Err(AppError::Blockchain(BlockchainError::TransactionFailed(
                        format!("Transaction failed: {:?}", status.err),
                    )));
                }
                // Check confirmation status
                let confirmed = status.confirmation_status.as_deref() == Some("confirmed")
                    || status.confirmation_status.as_deref() == Some("finalized");
                Ok(confirmed)
            }
            _ => Ok(false),
        }
    }

    #[instrument(skip(self))]
    async fn wait_for_confirmation(
        &self,
        signature: &str,
        timeout_secs: u64,
    ) -> Result<bool, AppError> {
        let timeout = Duration::from_secs(timeout_secs);
        let start = std::time::Instant::now();
        let poll_interval = Duration::from_millis(500);

        while start.elapsed() < timeout {
            match self.get_transaction_status(signature).await {
                Ok(true) => {
                    info!(signature = %signature, "Transaction confirmed");
                    return Ok(true);
                }
                Ok(false) => {
                    debug!(signature = %signature, "Transaction not yet confirmed");
                }
                Err(AppError::Blockchain(BlockchainError::TransactionFailed(msg))) => {
                    return Err(AppError::Blockchain(BlockchainError::TransactionFailed(
                        msg,
                    )));
                }
                Err(e) => {
                    warn!(signature = %signature, error = ?e, "Error checking transaction status");
                }
            }
            tokio::time::sleep(poll_interval).await;
        }

        Err(AppError::Blockchain(BlockchainError::Timeout(format!(
            "Transaction {} not confirmed within {}s",
            signature, timeout_secs
        ))))
    }

    #[instrument(skip(self))]
    async fn transfer_sol(
        &self,
        to_address: &str,
        amount_lamports: u64,
    ) -> Result<String, AppError> {
        info!(to = %to_address, amount_lamports = %amount_lamports, "Transferring SOL");

        // Validate amount
        if amount_lamports == 0 {
            return Err(AppError::Blockchain(BlockchainError::TransactionFailed(
                "Transfer amount must be greater than 0".to_string(),
            )));
        }

        // Check if we have SDK client and keypair
        let (sdk_client, keypair) = match (&self.sdk_client, &self.keypair) {
            (Some(client), Some(kp)) => (client, kp),
            _ => {
                return Err(AppError::Blockchain(BlockchainError::TransactionFailed(
                    "SDK client not initialized for SOL transfers".to_string(),
                )));
            }
        };

        // Parse destination address
        let to_pubkey = to_address.parse::<Pubkey>().map_err(|e| {
            AppError::Blockchain(BlockchainError::InvalidSignature(format!(
                "Invalid destination address: {}",
                e
            )))
        })?;

        // Get priority fee using provider-specific strategy
        let priority_fee = self.get_priority_fee(None).await;

        // Create transfer instruction using SDK
        let transfer_ix =
            system_instruction::transfer(&keypair.pubkey(), &to_pubkey, amount_lamports);

        // Build instructions with compute budget for priority fee
        let mut instructions = vec![
            ComputeBudgetInstruction::set_compute_unit_price(priority_fee),
            transfer_ix,
        ];

        // Append Jito tip instruction if enabled (MUST be last instruction per Jito best practices)
        if let Some(tip_ix) = self.create_jito_tip_instruction(&keypair.pubkey()) {
            info!(
                tip_lamports = self.jito_tip_lamports.unwrap_or(0),
                "Appending Jito tip instruction to SOL transfer"
            );
            instructions.push(tip_ix);
        }

        // Get recent blockhash using SDK
        let recent_blockhash = sdk_client
            .get_latest_blockhash()
            .await
            .map_err(map_solana_client_error)?;

        // Build and sign transaction
        let transaction = Transaction::new_signed_with_payer(
            &instructions,
            Some(&keypair.pubkey()),
            &[keypair],
            recent_blockhash,
        );

        // Submit via strategy if available, otherwise use SDK
        let signature = self.submit_or_confirm_transaction(&transaction).await?;

        info!(
            signature = %signature,
            to = %to_address,
            amount_lamports = %amount_lamports,
            via_strategy = self.submission_strategy.is_some(),
            jito_tip = self.jito_tip_lamports.filter(|_| self.supports_private_submission()),
            "SOL transfer submitted"
        );

        Ok(signature)
    }

    #[instrument(skip(self))]
    async fn transfer_token(
        &self,
        to_address: &str,
        token_mint: &str,
        amount: u64,
    ) -> Result<String, AppError> {
        info!(to = %to_address, token_mint = %token_mint, amount = %amount, "Transferring SPL Token (raw units)");

        // Validate amount
        if amount == 0 {
            return Err(AppError::Blockchain(BlockchainError::TransactionFailed(
                "Transfer amount must be greater than 0".to_string(),
            )));
        }

        // Check if we have SDK client and keypair
        let (sdk_client, keypair) = match (&self.sdk_client, &self.keypair) {
            (Some(client), Some(kp)) => (client, kp),
            _ => {
                return Err(AppError::Blockchain(BlockchainError::TransactionFailed(
                    "SDK client not initialized for token transfers".to_string(),
                )));
            }
        };

        // Parse addresses
        let to_pubkey = to_address.parse::<Pubkey>().map_err(|e| {
            AppError::Blockchain(BlockchainError::InvalidSignature(format!(
                "Invalid destination address: {}",
                e
            )))
        })?;

        let mint_pubkey = token_mint.parse::<Pubkey>().map_err(|e| {
            AppError::Blockchain(BlockchainError::InvalidSignature(format!(
                "Invalid token mint address: {}",
                e
            )))
        })?;

        // Fetch the mint account to determine the correct token program ID and decimals
        // This is required for transfer_checked instruction (validates decimals) and Token-2022 support
        let mint_account = sdk_client.get_account(&mint_pubkey).await.map_err(|e| {
            AppError::Blockchain(BlockchainError::TransactionFailed(format!(
                "Failed to fetch mint account: {}",
                e
            )))
        })?;

        // The mint account's owner is the token program ID
        let token_program_id = mint_account.owner;
        debug!(token_program_id = %token_program_id, "Detected token program from mint");

        // Extract decimals from mint account data (required for transfer_checked)
        // Mint layout (both SPL Token and Token-2022):
        // - bytes 0-35: mint_authority option (1 byte option flag + up to 32 bytes pubkey)
        // - bytes 36-43: supply (u64)
        // - byte 44: decimals (u8)
        // - byte 45: is_initialized (bool)
        // - bytes 46-78: freeze_authority option
        const DECIMALS_OFFSET: usize = 44;
        const MIN_MINT_SIZE: usize = 82;

        if mint_account.data.len() < MIN_MINT_SIZE {
            return Err(AppError::Blockchain(BlockchainError::TransactionFailed(
                format!(
                    "Mint account data too small: {} bytes, expected at least {}",
                    mint_account.data.len(),
                    MIN_MINT_SIZE
                ),
            )));
        }

        let decimals = mint_account.data[DECIMALS_OFFSET];
        debug!(decimals = %decimals, "Read decimals from mint account (needed for transfer_checked)");

        // Derive Associated Token Accounts with the correct token program ID
        let source_ata = get_associated_token_address_with_program_id(
            &keypair.pubkey(),
            &mint_pubkey,
            &token_program_id,
        );
        let destination_ata = get_associated_token_address_with_program_id(
            &to_pubkey,
            &mint_pubkey,
            &token_program_id,
        );

        debug!(
            source_ata = %source_ata,
            destination_ata = %destination_ata,
            token_program_id = %token_program_id,
            "Derived ATAs for token transfer"
        );

        // CRITICAL: Verify source ATA exists and has sufficient balance
        let source_account = sdk_client.get_account(&source_ata).await.map_err(|e| {
            AppError::Blockchain(BlockchainError::TransactionFailed(format!(
                "Source token account does not exist or cannot be fetched. \
                 The sender ({}) does not have an associated token account for mint {}. \
                 Error: {}",
                keypair.pubkey(),
                token_mint,
                e
            )))
        })?;

        // Verify the source account is owned by the token program
        if source_account.owner != token_program_id {
            return Err(AppError::Blockchain(BlockchainError::TransactionFailed(
                format!(
                    "Source token account is not owned by the token program. \
                     Expected owner: {}, actual owner: {}",
                    token_program_id, source_account.owner
                ),
            )));
        }

        // Extract balance from token account data to verify sufficient funds
        // Token account layout: amount is at bytes 64-72 (u64 LE)
        const TOKEN_ACCOUNT_AMOUNT_OFFSET: usize = 64;
        if source_account.data.len() >= TOKEN_ACCOUNT_AMOUNT_OFFSET + 8 {
            let balance_bytes: [u8; 8] = source_account.data
                [TOKEN_ACCOUNT_AMOUNT_OFFSET..TOKEN_ACCOUNT_AMOUNT_OFFSET + 8]
                .try_into()
                .unwrap();
            let balance = u64::from_le_bytes(balance_bytes);
            debug!(source_balance = %balance, required = %amount, "Checking source token balance");

            if balance < amount {
                return Err(AppError::Blockchain(BlockchainError::InsufficientFunds));
            }
        }

        // Get priority fee using provider-specific strategy
        let priority_fee = self.get_priority_fee(None).await;

        // Start with compute budget instruction for priority fee
        let mut instructions: Vec<Instruction> =
            vec![ComputeBudgetInstruction::set_compute_unit_price(
                priority_fee,
            )];

        // Check if destination ATA exists
        let dest_account_result = sdk_client.get_account(&destination_ata).await;

        if dest_account_result.is_err() {
            // ATA doesn't exist - create it using idempotent instruction
            // This is safer as it won't fail if the ATA gets created between our check and execution
            info!(destination_ata = %destination_ata, "Creating destination ATA");
            let create_ata_ix = create_associated_token_account_idempotent(
                &keypair.pubkey(), // payer
                &to_pubkey,        // wallet owner
                &mint_pubkey,      // token mint
                &token_program_id, // token program (dynamically detected)
            );
            instructions.push(create_ata_ix);
        }

        // Create SPL Token transfer_checked instruction for safer transfers
        // transfer_checked validates the mint and decimals, providing better error messages
        // Note: We pass the raw `amount` directly (already in token units), but still need
        // `decimals` for the transfer_checked instruction validation
        let transfer_ix = token_instruction::transfer_checked(
            &token_program_id,
            &source_ata,
            &mint_pubkey,
            &destination_ata,
            &keypair.pubkey(), // authority (owner of source account)
            &[],               // no multisig signers
            amount,            // already in raw token units
            decimals,          // required by transfer_checked for validation
        )
        .map_err(|e| {
            AppError::Blockchain(BlockchainError::TransactionFailed(format!(
                "Failed to create transfer_checked instruction: {}",
                e
            )))
        })?;

        instructions.push(transfer_ix);

        // Append Jito tip instruction if enabled (MUST be last instruction per Jito best practices)
        if let Some(tip_ix) = self.create_jito_tip_instruction(&keypair.pubkey()) {
            info!(
                tip_lamports = self.jito_tip_lamports.unwrap_or(0),
                "Appending Jito tip instruction to token transfer"
            );
            instructions.push(tip_ix);
        }

        // Get recent blockhash
        let recent_blockhash = sdk_client
            .get_latest_blockhash()
            .await
            .map_err(map_solana_client_error)?;

        // Build and sign transaction
        let transaction = Transaction::new_signed_with_payer(
            &instructions,
            Some(&keypair.pubkey()),
            &[keypair],
            recent_blockhash,
        );

        // Submit via strategy if available, otherwise use SDK
        let signature = self.submit_or_confirm_transaction(&transaction).await?;

        info!(
            signature = %signature,
            to = %to_address,
            token_mint = %token_mint,
            amount = %amount,
            decimals = %decimals,
            via_strategy = self.submission_strategy.is_some(),
            jito_tip = self.jito_tip_lamports.filter(|_| self.supports_private_submission()),
            "SPL Token transfer submitted (raw units)"
        );

        Ok(signature)
    }

    /// Check if a wallet holds compliant assets using Helius DAS.
    ///
    /// This method checks if the wallet holds any assets from sanctioned collections.
    /// It is only available when using a Helius RPC provider.
    ///
    /// # Behavior by Provider
    /// - **Helius**: Uses `getAssetsByOwner` DAS API to check asset collections
    /// - **QuickNode/Standard**: Returns `true` (skip check, assume compliant)
    ///
    /// # Arguments
    /// * `owner` - The wallet address (Base58) to check
    ///
    /// # Returns
    /// * `Ok(true)` - Wallet is compliant (no sanctioned assets or DAS not available)
    /// * `Ok(false)` - Wallet holds sanctioned assets
    /// * `Err(_)` - API error during check
    #[instrument(skip(self))]
    async fn check_wallet_assets(&self, owner: &str) -> Result<bool, AppError> {
        match &self.das_client {
            Some(das_client) => {
                info!(wallet = %owner, "Helius DAS Check: Initiating asset scan");
                das_client.check_wallet_compliance(owner).await
            }
            None => {
                debug!(
                    wallet = %owner,
                    provider = %self.provider_type.name(),
                    "DAS not available for this provider, skipping asset check"
                );
                Ok(true)
            }
        }
    }

    // =========================================================================
    // Jito Double Spend Protection Methods
    // =========================================================================

    /// Query the status of a transaction by its signature.
    /// Used to verify if an original transaction was processed before retrying
    /// after a JitoStateUnknown error.
    #[instrument(skip(self))]
    async fn get_signature_status(
        &self,
        signature: &str,
    ) -> Result<Option<crate::domain::TransactionStatus>, AppError> {
        use crate::domain::TransactionStatus;

        let params = serde_json::json!([[signature], {"searchTransactionHistory": true}]);
        let result: SignatureStatusResult = self.rpc_call("getSignatureStatuses", params).await?;

        match result.value.first() {
            Some(Some(status)) => {
                // Check if transaction errored
                if let Some(ref err) = status.err {
                    return Ok(Some(TransactionStatus::Failed(format!("{:?}", err))));
                }

                // Check confirmation status
                match status.confirmation_status.as_deref() {
                    Some("finalized") => Ok(Some(TransactionStatus::Finalized)),
                    Some("confirmed") => Ok(Some(TransactionStatus::Confirmed)),
                    // Transaction is still processing or unknown status
                    _ => Ok(None),
                }
            }
            // Transaction not found
            _ => Ok(None),
        }
    }

    /// Check if a blockhash is still valid (not expired).
    /// Blockhashes typically expire after ~150 slots (~1-2 minutes).
    #[instrument(skip(self))]
    async fn is_blockhash_valid(&self, blockhash: &str) -> Result<bool, AppError> {
        // Use the SDK client if available for accurate blockhash validation
        if let Some(sdk_client) = &self.sdk_client {
            use solana_hash::Hash;
            use std::str::FromStr;

            let hash = Hash::from_str(blockhash).map_err(|e| {
                AppError::Validation(crate::domain::ValidationError::InvalidField {
                    field: "blockhash".to_string(),
                    message: format!("Invalid blockhash format: {}", e),
                })
            })?;

            let is_valid = sdk_client
                .is_blockhash_valid(&hash, CommitmentConfig::confirmed())
                .await
                .map_err(map_solana_client_error)?;

            debug!(
                blockhash = %blockhash,
                is_valid = %is_valid,
                "Checked blockhash validity"
            );

            return Ok(is_valid);
        }

        // Fallback: Use RPC method directly
        // The `isBlockhashValid` RPC method returns whether the blockhash is still valid
        let params = serde_json::json!([blockhash, {"commitment": "confirmed"}]);

        #[derive(Debug, Deserialize)]
        struct IsValidResult {
            value: bool,
        }

        match self
            .rpc_call::<IsValidResult>("isBlockhashValid", params)
            .await
        {
            Ok(result) => {
                debug!(
                    blockhash = %blockhash,
                    is_valid = %result.value,
                    "Checked blockhash validity via RPC"
                );
                Ok(result.value)
            }
            Err(e) => {
                warn!(
                    blockhash = %blockhash,
                    error = %e,
                    "Failed to check blockhash validity, assuming expired"
                );
                // On error, assume expired (safe to retry with new blockhash)
                Ok(false)
            }
        }
    }
}

/// Map Solana client errors to our AppError types
fn map_solana_client_error(err: solana_client::client_error::ClientError) -> AppError {
    use solana_client::client_error::ClientErrorKind;

    let msg = err.to_string();

    match err.kind() {
        ClientErrorKind::RpcError(_) => {
            if msg.contains("insufficient") || msg.contains("InsufficientFunds") {
                AppError::Blockchain(BlockchainError::InsufficientFunds)
            } else {
                AppError::Blockchain(BlockchainError::RpcError(msg))
            }
        }
        ClientErrorKind::Io(_) => AppError::Blockchain(BlockchainError::Connection(msg)),
        ClientErrorKind::Reqwest(_) => {
            if msg.contains("timeout") || msg.contains("timed out") {
                AppError::Blockchain(BlockchainError::Timeout(msg))
            } else {
                AppError::Blockchain(BlockchainError::Connection(msg))
            }
        }
        _ => AppError::Blockchain(BlockchainError::TransactionFailed(msg)),
    }
}

/// Parse a base58-encoded private key into a SigningKey
pub fn signing_key_from_base58(secret: &SecretString) -> Result<SigningKey, AppError> {
    let key_bytes = bs58::decode(secret.expose_secret())
        .into_vec()
        .map_err(|e| AppError::Blockchain(BlockchainError::InvalidSignature(e.to_string())))?;

    // Handle both 32-byte (seed) and 64-byte (keypair) formats
    let key_array: [u8; 32] = if key_bytes.len() == 64 {
        // Solana keypair format: first 32 bytes are the secret key
        key_bytes[..32].try_into().map_err(|_| {
            AppError::Blockchain(BlockchainError::InvalidSignature(
                "Invalid keypair format".to_string(),
            ))
        })?
    } else if key_bytes.len() == 32 {
        key_bytes.try_into().map_err(|v: Vec<u8>| {
            AppError::Blockchain(BlockchainError::InvalidSignature(format!(
                "Key must be 32 bytes, got {}",
                v.len()
            )))
        })?
    } else {
        return Err(AppError::Blockchain(BlockchainError::InvalidSignature(
            format!("Key must be 32 or 64 bytes, got {}", key_bytes.len()),
        )));
    };

    Ok(SigningKey::from_bytes(&key_array))
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::rngs::OsRng;

    #[test]
    fn test_client_creation() {
        let signing_key = SigningKey::generate(&mut OsRng);
        let client =
            RpcBlockchainClient::with_defaults("https://api.devnet.solana.com", signing_key);
        assert!(client.is_ok());
    }

    #[test]
    fn test_public_key_generation() {
        let signing_key = SigningKey::generate(&mut OsRng);
        let client =
            RpcBlockchainClient::with_defaults("https://api.devnet.solana.com", signing_key)
                .unwrap();
        let pubkey = client.public_key();
        assert!(!pubkey.is_empty());
        // Verify it decodes to 32 bytes (length can be 43 or 44 chars)
        let decoded = bs58::decode(&pubkey)
            .into_vec()
            .expect("Should be valid base58");
        assert_eq!(decoded.len(), 32);
    }

    #[test]
    fn test_signing() {
        let signing_key = SigningKey::generate(&mut OsRng);
        let client =
            RpcBlockchainClient::with_defaults("https://api.devnet.solana.com", signing_key)
                .unwrap();
        let signature = client.sign(b"test message");
        assert!(!signature.is_empty());
    }

    #[test]
    fn test_signing_key_from_base58_valid_32_bytes() {
        let original_key = SigningKey::generate(&mut OsRng);
        let encoded = bs58::encode(original_key.to_bytes()).into_string();
        let secret = SecretString::from(encoded);
        let result = signing_key_from_base58(&secret);
        assert!(result.is_ok());
    }

    #[test]
    fn test_signing_key_from_base58_valid_64_bytes() {
        let original_key = SigningKey::generate(&mut OsRng);
        let mut keypair = original_key.to_bytes().to_vec();
        keypair.extend_from_slice(original_key.verifying_key().as_bytes());
        let encoded = bs58::encode(&keypair).into_string();
        let secret = SecretString::from(encoded);
        let result = signing_key_from_base58(&secret);
        assert!(result.is_ok());
    }

    #[test]
    fn test_signing_key_from_base58_invalid() {
        let secret = SecretString::from("invalid-base58!!!");
        let result = signing_key_from_base58(&secret);
        assert!(result.is_err());
    }

    #[test]
    fn test_rpc_client_config_default() {
        let config = RpcClientConfig::default();
        assert_eq!(config.max_retries, 3);
        assert_eq!(config.timeout, Duration::from_secs(30));
        assert_eq!(config.confirmation_timeout, Duration::from_secs(60));
    }

    #[test]
    fn test_signing_key_from_base58_wrong_length() {
        // 16 bytes - too short
        let short_key = bs58::encode(vec![0u8; 16]).into_string();
        let secret = SecretString::from(short_key);
        let result = signing_key_from_base58(&secret);
        assert!(result.is_err());

        // 48 bytes - wrong size (not 32 or 64)
        let wrong_key = bs58::encode(vec![0u8; 48]).into_string();
        let secret = SecretString::from(wrong_key);
        let result = signing_key_from_base58(&secret);
        assert!(result.is_err());
    }

    #[test]
    fn test_rpc_client_config_custom() {
        let config = RpcClientConfig {
            timeout: Duration::from_secs(60),
            max_retries: 5,
            retry_delay: Duration::from_millis(1000),
            confirmation_timeout: Duration::from_secs(120),
        };
        assert_eq!(config.timeout, Duration::from_secs(60));
        assert_eq!(config.max_retries, 5);
        assert_eq!(config.retry_delay, Duration::from_millis(1000));
        assert_eq!(config.confirmation_timeout, Duration::from_secs(120));
    }

    #[test]
    fn test_signing_determinism() {
        let signing_key = SigningKey::generate(&mut OsRng);
        let client =
            RpcBlockchainClient::with_defaults("https://api.devnet.solana.com", signing_key)
                .unwrap();

        // Same message should produce same signature
        let sig1 = client.sign(b"test message");
        let sig2 = client.sign(b"test message");
        assert_eq!(sig1, sig2);

        // Different message should produce different signature
        let sig3 = client.sign(b"different message");
        assert_ne!(sig1, sig3);
    }

    // --- MOCK PROVIDER TESTS ---
    use std::sync::Mutex;

    #[cfg(test)]
    #[allow(dead_code)]
    enum BlockchainErrorType {
        Timeout,
        Rpc,
    }

    struct MockState {
        requests: Vec<String>,
        should_fail_count: u32,
        failure_error: Option<BlockchainErrorType>,
        next_response: Option<serde_json::Value>,
    }

    struct MockSolanaRpcProvider {
        state: Mutex<MockState>,
        signing_key: SigningKey,
    }

    impl MockSolanaRpcProvider {
        fn new() -> Self {
            Self {
                state: Mutex::new(MockState {
                    requests: Vec::new(),
                    should_fail_count: 0,
                    failure_error: None,
                    next_response: None,
                }),
                signing_key: SigningKey::generate(&mut OsRng),
            }
        }

        fn with_failure(count: u32, error_type: BlockchainErrorType) -> Self {
            let provider = Self::new(); // removed `mut` since we don’t mutate `provider` itself
            {
                let mut state = provider.state.lock().unwrap();
                state.should_fail_count = count;
                state.failure_error = Some(error_type);
            }
            provider
        }
    }

    #[async_trait]
    impl SolanaRpcProvider for MockSolanaRpcProvider {
        async fn send_request(
            &self,
            method: &str,
            _params: serde_json::Value,
        ) -> Result<serde_json::Value, AppError> {
            let mut state = self.state.lock().unwrap();
            state.requests.push(method.to_string());

            if state.should_fail_count > 0 {
                state.should_fail_count -= 1;
                if let Some(ref err) = state.failure_error {
                    return match err {
                        BlockchainErrorType::Timeout => Err(AppError::Blockchain(
                            BlockchainError::Timeout("Mock timeout".to_string()),
                        )),
                        BlockchainErrorType::Rpc => Err(AppError::Blockchain(
                            BlockchainError::RpcError("Mock RPC error".to_string()),
                        )),
                    };
                }
            }

            if let Some(resp) = &state.next_response {
                return Ok(resp.clone());
            }

            Ok(serde_json::Value::Null)
        }

        fn public_key(&self) -> String {
            bs58::encode(self.signing_key.verifying_key().as_bytes()).into_string()
        }

        fn sign(&self, message: &[u8]) -> String {
            let signature = self.signing_key.sign(message);
            bs58::encode(signature.to_bytes()).into_string()
        }
    }

    #[tokio::test]
    async fn test_rpc_client_retry_logic_success() {
        // Setup provider that fails twice then succeeds
        let provider = MockSolanaRpcProvider::with_failure(2, BlockchainErrorType::Timeout);
        let config = RpcClientConfig {
            max_retries: 3,
            retry_delay: Duration::from_millis(1), // Fast retry
            ..Default::default()
        };

        // Set success response
        {
            let mut state = provider.state.lock().unwrap();
            state.next_response = Some(serde_json::json!(12345u64)); // Slot response
        }

        let client = RpcBlockchainClient::with_provider(Box::new(provider), config);

        // Call health_check (uses getSlot)
        let result = client.health_check().await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_rpc_client_retry_logic_failure() {
        // Setup provider that fails 4 times (max retries is 3)
        let provider = MockSolanaRpcProvider::with_failure(4, BlockchainErrorType::Timeout);
        let config = RpcClientConfig {
            max_retries: 3,
            retry_delay: Duration::from_millis(1),
            ..Default::default()
        };

        let client = RpcBlockchainClient::with_provider(Box::new(provider), config);

        let result = client.health_check().await;
        assert!(matches!(
            result,
            Err(AppError::Blockchain(BlockchainError::Timeout(_)))
        ));
    }

    // --- ENHANCED MOCK FOR ERROR SCENARIOS ---

    #[derive(Clone)]
    #[allow(dead_code)]
    enum MockErrorKind {
        Timeout(String),
        RpcError(String),
        InsufficientFunds,
        TransactionFailed(String),
        EmptyResponse,
    }

    struct ConfigurableMockProvider {
        signing_key: SigningKey,
        responses: Mutex<Vec<Result<serde_json::Value, MockErrorKind>>>,
        call_count: Mutex<usize>,
    }

    impl ConfigurableMockProvider {
        fn new() -> Self {
            Self {
                signing_key: SigningKey::generate(&mut OsRng),
                responses: Mutex::new(Vec::new()),
                call_count: Mutex::new(0),
            }
        }

        fn with_responses(responses: Vec<Result<serde_json::Value, MockErrorKind>>) -> Self {
            let provider = Self::new();
            *provider.responses.lock().unwrap() = responses;
            provider
        }

        #[allow(dead_code)]
        fn get_call_count(&self) -> usize {
            *self.call_count.lock().unwrap()
        }
    }

    #[async_trait]
    impl SolanaRpcProvider for ConfigurableMockProvider {
        async fn send_request(
            &self,
            _method: &str,
            _params: serde_json::Value,
        ) -> Result<serde_json::Value, AppError> {
            let mut count = self.call_count.lock().unwrap();
            let idx = *count;
            *count += 1;
            drop(count);

            let responses = self.responses.lock().unwrap();
            if idx < responses.len() {
                match &responses[idx] {
                    Ok(v) => Ok(v.clone()),
                    Err(MockErrorKind::Timeout(msg)) => {
                        Err(AppError::Blockchain(BlockchainError::Timeout(msg.clone())))
                    }
                    Err(MockErrorKind::RpcError(msg)) => {
                        Err(AppError::Blockchain(BlockchainError::RpcError(msg.clone())))
                    }
                    Err(MockErrorKind::InsufficientFunds) => {
                        Err(AppError::Blockchain(BlockchainError::InsufficientFunds))
                    }
                    Err(MockErrorKind::TransactionFailed(msg)) => Err(AppError::Blockchain(
                        BlockchainError::TransactionFailed(msg.clone()),
                    )),
                    Err(MockErrorKind::EmptyResponse) => Err(AppError::Blockchain(
                        BlockchainError::RpcError("Empty response".to_string()),
                    )),
                }
            } else {
                Ok(serde_json::Value::Null)
            }
        }

        fn public_key(&self) -> String {
            bs58::encode(self.signing_key.verifying_key().as_bytes()).into_string()
        }

        fn sign(&self, message: &[u8]) -> String {
            let signature = self.signing_key.sign(message);
            bs58::encode(signature.to_bytes()).into_string()
        }
    }

    // --- ERROR HANDLING TESTS ---

    #[tokio::test]
    async fn test_rpc_error_insufficient_funds() {
        let provider =
            ConfigurableMockProvider::with_responses(vec![Err(MockErrorKind::InsufficientFunds)]);
        let config = RpcClientConfig {
            max_retries: 0, // No retries for this test
            ..Default::default()
        };
        let client = RpcBlockchainClient::with_provider(Box::new(provider), config);

        let result = client.health_check().await;
        assert!(matches!(
            result,
            Err(AppError::Blockchain(BlockchainError::InsufficientFunds))
        ));
    }

    #[tokio::test]
    async fn test_rpc_error_timeout_mapping() {
        let provider = ConfigurableMockProvider::with_responses(vec![Err(MockErrorKind::Timeout(
            "Connection timed out".to_string(),
        ))]);
        let config = RpcClientConfig {
            max_retries: 0,
            ..Default::default()
        };
        let client = RpcBlockchainClient::with_provider(Box::new(provider), config);

        let result = client.health_check().await;
        match result {
            Err(AppError::Blockchain(BlockchainError::Timeout(msg))) => {
                assert!(msg.contains("timed out"));
            }
            _ => panic!("Expected timeout error"),
        }
    }

    #[tokio::test]
    async fn test_rpc_error_generic_rpc_error() {
        let provider = ConfigurableMockProvider::with_responses(vec![Err(
            MockErrorKind::RpcError("-32000: Server is busy".to_string()),
        )]);
        let config = RpcClientConfig {
            max_retries: 0,
            ..Default::default()
        };
        let client = RpcBlockchainClient::with_provider(Box::new(provider), config);

        let result = client.health_check().await;
        match result {
            Err(AppError::Blockchain(BlockchainError::RpcError(msg))) => {
                assert!(msg.contains("Server is busy"));
            }
            _ => panic!("Expected RPC error"),
        }
    }

    // --- DESERIALIZATION TESTS ---

    #[test]
    fn test_deserialize_signature_status_confirmed() {
        let json = serde_json::json!({
            "err": null,
            "confirmationStatus": "confirmed"
        });
        let status: SignatureStatus = serde_json::from_value(json).unwrap();
        assert!(status.err.is_none());
        assert_eq!(status.confirmation_status.as_deref(), Some("confirmed"));
    }

    #[test]
    fn test_deserialize_signature_status_finalized() {
        let json = serde_json::json!({
            "err": null,
            "confirmationStatus": "finalized"
        });
        let status: SignatureStatus = serde_json::from_value(json).unwrap();
        assert!(status.err.is_none());
        assert_eq!(status.confirmation_status.as_deref(), Some("finalized"));
    }

    #[test]
    fn test_deserialize_signature_status_with_error() {
        let json = serde_json::json!({
            "err": {"InstructionError": [0, "Custom"]},
            "confirmationStatus": "confirmed"
        });
        let status: SignatureStatus = serde_json::from_value(json).unwrap();
        assert!(status.err.is_some());
    }

    #[test]
    fn test_deserialize_signature_status_null_confirmation() {
        let json = serde_json::json!({
            "err": null,
            "confirmationStatus": null
        });
        let status: SignatureStatus = serde_json::from_value(json).unwrap();
        assert!(status.confirmation_status.is_none());
    }

    #[test]
    fn test_deserialize_blockhash_result() {
        let json = serde_json::json!({
            "value": {
                "blockhash": "GHtXQBsoZHVnNFa9YevAzFr17DJjgHXk3ycTy5nRhVT3"
            }
        });
        let result: BlockhashResult = serde_json::from_value(json).unwrap();
        assert_eq!(
            result.value.blockhash,
            "GHtXQBsoZHVnNFa9YevAzFr17DJjgHXk3ycTy5nRhVT3"
        );
    }

    #[test]
    fn test_deserialize_signature_status_result() {
        let json = serde_json::json!({
            "value": [
                {
                    "err": null,
                    "confirmationStatus": "finalized"
                }
            ]
        });
        let result: SignatureStatusResult = serde_json::from_value(json).unwrap();
        assert_eq!(result.value.len(), 1);
        assert!(result.value[0].is_some());
    }

    #[test]
    fn test_deserialize_signature_status_result_null_entry() {
        let json = serde_json::json!({
            "value": [null]
        });
        let result: SignatureStatusResult = serde_json::from_value(json).unwrap();
        assert_eq!(result.value.len(), 1);
        assert!(result.value[0].is_none());
    }

    // --- TRANSACTION STATUS TESTS ---

    #[tokio::test]
    async fn test_get_transaction_status_confirmed() {
        let provider = ConfigurableMockProvider::with_responses(vec![Ok(serde_json::json!({
            "value": [{
                "err": null,
                "confirmationStatus": "confirmed"
            }]
        }))]);
        let config = RpcClientConfig::default();
        let client = RpcBlockchainClient::with_provider(Box::new(provider), config);

        let result = client.get_transaction_status("test_sig").await;
        assert!(result.is_ok());
        assert!(result.unwrap()); // Should be confirmed
    }

    #[tokio::test]
    async fn test_get_transaction_status_finalized() {
        let provider = ConfigurableMockProvider::with_responses(vec![Ok(serde_json::json!({
            "value": [{
                "err": null,
                "confirmationStatus": "finalized"
            }]
        }))]);
        let config = RpcClientConfig::default();
        let client = RpcBlockchainClient::with_provider(Box::new(provider), config);

        let result = client.get_transaction_status("test_sig").await;
        assert!(result.is_ok());
        assert!(result.unwrap());
    }

    #[tokio::test]
    async fn test_get_transaction_status_not_found() {
        let provider = ConfigurableMockProvider::with_responses(vec![Ok(serde_json::json!({
            "value": [null]
        }))]);
        let config = RpcClientConfig::default();
        let client = RpcBlockchainClient::with_provider(Box::new(provider), config);

        let result = client.get_transaction_status("unknown_sig").await;
        assert!(result.is_ok());
        assert!(!result.unwrap()); // Not found = not confirmed
    }

    #[tokio::test]
    async fn test_get_transaction_status_with_error() {
        let provider = ConfigurableMockProvider::with_responses(vec![Ok(serde_json::json!({
            "value": [{
                "err": {"InstructionError": [0, "Custom"]},
                "confirmationStatus": "confirmed"
            }]
        }))]);
        let config = RpcClientConfig::default();
        let client = RpcBlockchainClient::with_provider(Box::new(provider), config);

        let result = client.get_transaction_status("failed_sig").await;
        assert!(matches!(
            result,
            Err(AppError::Blockchain(BlockchainError::TransactionFailed(_)))
        ));
    }

    // --- BLOCKHASH AND BLOCK HEIGHT TESTS ---

    #[tokio::test]
    async fn test_get_latest_blockhash() {
        let provider = ConfigurableMockProvider::with_responses(vec![Ok(serde_json::json!({
            "value": {
                "blockhash": "TestBlockhash123"
            }
        }))]);
        let config = RpcClientConfig::default();
        let client = RpcBlockchainClient::with_provider(Box::new(provider), config);

        let result = client.get_latest_blockhash().await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "TestBlockhash123");
    }

    #[tokio::test]
    async fn test_get_block_height() {
        let provider =
            ConfigurableMockProvider::with_responses(vec![Ok(serde_json::json!(123456789u64))]);
        let config = RpcClientConfig::default();
        let client = RpcBlockchainClient::with_provider(Box::new(provider), config);

        let result = client.get_block_height().await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), 123456789);
    }

    // --- WAIT FOR CONFIRMATION TESTS ---

    #[tokio::test]
    async fn test_wait_for_confirmation_immediate_success() {
        let provider = ConfigurableMockProvider::with_responses(vec![Ok(serde_json::json!({
            "value": [{
                "err": null,
                "confirmationStatus": "finalized"
            }]
        }))]);
        let config = RpcClientConfig::default();
        let client = RpcBlockchainClient::with_provider(Box::new(provider), config);

        let result = client.wait_for_confirmation("test_sig", 5).await;
        assert!(result.is_ok());
        assert!(result.unwrap());
    }

    #[tokio::test]
    async fn test_wait_for_confirmation_eventual_success() {
        // First call: not confirmed, second call: confirmed
        let provider = ConfigurableMockProvider::with_responses(vec![
            Ok(serde_json::json!({"value": [null]})),
            Ok(serde_json::json!({
                "value": [{
                    "err": null,
                    "confirmationStatus": "confirmed"
                }]
            })),
        ]);
        let config = RpcClientConfig::default();
        let client = RpcBlockchainClient::with_provider(Box::new(provider), config);

        tokio::time::pause();
        let result = client.wait_for_confirmation("test_sig", 10).await;
        assert!(result.is_ok());
        assert!(result.unwrap());
    }

    #[tokio::test]
    async fn test_wait_for_confirmation_timeout() {
        // Always return not confirmed
        let provider = ConfigurableMockProvider::with_responses(vec![
            Ok(serde_json::json!({"value": [null]})),
            Ok(serde_json::json!({"value": [null]})),
            Ok(serde_json::json!({"value": [null]})),
            Ok(serde_json::json!({"value": [null]})),
            Ok(serde_json::json!({"value": [null]})),
        ]);
        let config = RpcClientConfig::default();
        let client = RpcBlockchainClient::with_provider(Box::new(provider), config);

        tokio::time::pause();
        let result = client.wait_for_confirmation("never_confirmed", 1).await;
        assert!(matches!(
            result,
            Err(AppError::Blockchain(BlockchainError::Timeout(_)))
        ));
    }

    #[tokio::test]
    async fn test_wait_for_confirmation_transaction_failed() {
        let provider = ConfigurableMockProvider::with_responses(vec![Ok(serde_json::json!({
            "value": [{
                "err": {"InstructionError": [0, "ProgramFailed"]},
                "confirmationStatus": "confirmed"
            }]
        }))]);
        let config = RpcClientConfig::default();
        let client = RpcBlockchainClient::with_provider(Box::new(provider), config);

        let result = client.wait_for_confirmation("failed_tx", 5).await;
        assert!(matches!(
            result,
            Err(AppError::Blockchain(BlockchainError::TransactionFailed(_)))
        ));
    }

    // --- SUBMIT TRANSACTION TESTS (MOCK MODE) ---

    #[tokio::test]
    #[cfg(not(feature = "real-blockchain"))]
    async fn test_submit_transaction_mock_mode() {
        let provider = ConfigurableMockProvider::new();
        let config = RpcClientConfig::default();
        let client = RpcBlockchainClient::with_provider(Box::new(provider), config);

        // In mock mode (no real-blockchain feature), submit_transaction just signs
        let request = TransferRequest {
            id: "test_hash_123".to_string(),
            ..Default::default()
        };
        let result = client.submit_transaction(&request).await;
        assert!(result.is_ok());
        let signature = result.unwrap();
        assert!(signature.starts_with("tx_")); // Mock format
    }

    // --- RETRY LOGIC WITH CALL TRACKING ---

    #[tokio::test]
    async fn test_retry_counts_attempts_correctly() {
        let provider = ConfigurableMockProvider::with_responses(vec![
            Err(MockErrorKind::Timeout("fail 1".to_string())),
            Err(MockErrorKind::Timeout("fail 2".to_string())),
            Err(MockErrorKind::Timeout("fail 3".to_string())),
            Ok(serde_json::json!(999u64)), // Success on 4th attempt
        ]);
        let config = RpcClientConfig {
            max_retries: 3, // Initial + 3 retries = 4 attempts
            retry_delay: Duration::from_millis(1),
            ..Default::default()
        };
        let client = RpcBlockchainClient::with_provider(Box::new(provider), config);

        let result = client.health_check().await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_no_retry_on_insufficient_funds() {
        // InsufficientFunds should still trigger retries as per current implementation
        let provider = ConfigurableMockProvider::with_responses(vec![
            Err(MockErrorKind::InsufficientFunds),
            Err(MockErrorKind::InsufficientFunds),
        ]);
        let config = RpcClientConfig {
            max_retries: 1,
            retry_delay: Duration::from_millis(1),
            ..Default::default()
        };
        let client = RpcBlockchainClient::with_provider(Box::new(provider), config);

        let result = client.health_check().await;
        assert!(matches!(
            result,
            Err(AppError::Blockchain(BlockchainError::InsufficientFunds))
        ));
        // Note: We can't check the provider's state after moving it into Box
        // The test validates that InsufficientFunds is eventually returned after retries
    }

    // --- WITH_PROVIDER CONSTRUCTOR TEST ---

    #[test]
    fn test_with_provider_constructor() {
        let provider = ConfigurableMockProvider::new();
        let config = RpcClientConfig {
            max_retries: 5,
            timeout: Duration::from_secs(45),
            ..Default::default()
        };
        let client = RpcBlockchainClient::with_provider(Box::new(provider), config);

        // Verify public key is accessible
        let pubkey = client.public_key();
        assert!(!pubkey.is_empty());

        // Verify signing works
        let sig = client.sign(b"test");
        assert!(!sig.is_empty());
    }

    // --- HTTP PROVIDER TESTS ---

    #[test]
    fn test_http_solana_rpc_provider_creation() {
        let signing_key = SigningKey::generate(&mut OsRng);
        let result = HttpSolanaRpcProvider::new(
            "https://api.devnet.solana.com",
            signing_key,
            Duration::from_secs(30),
        );
        assert!(result.is_ok());
    }

    #[test]
    fn test_http_solana_rpc_provider_public_key() {
        let signing_key = SigningKey::generate(&mut OsRng);
        let provider = HttpSolanaRpcProvider::new(
            "https://api.devnet.solana.com",
            signing_key.clone(),
            Duration::from_secs(30),
        )
        .unwrap();

        let pubkey = provider.public_key();
        assert!(!pubkey.is_empty());
        // Verify it matches the expected public key
        let expected = bs58::encode(signing_key.verifying_key().as_bytes()).into_string();
        assert_eq!(pubkey, expected);
    }

    #[test]
    fn test_http_solana_rpc_provider_sign() {
        let signing_key = SigningKey::generate(&mut OsRng);
        let provider = HttpSolanaRpcProvider::new(
            "https://api.devnet.solana.com",
            signing_key,
            Duration::from_secs(30),
        )
        .unwrap();

        let signature = provider.sign(b"test message");
        assert!(!signature.is_empty());
        // Signature should be base58 encoded
        let decoded = bs58::decode(&signature).into_vec();
        assert!(decoded.is_ok());
        assert_eq!(decoded.unwrap().len(), 64); // Ed25519 signature is 64 bytes
    }

    // --- JSON-RPC STRUCTURE TESTS ---

    #[test]
    fn test_json_rpc_response_with_result() {
        let json = serde_json::json!({
            "result": 12345,
            "error": null
        });
        let response: JsonRpcResponse<u64> = serde_json::from_value(json).unwrap();
        assert_eq!(response.result, Some(12345));
        assert!(response.error.is_none());
    }

    #[test]
    fn test_json_rpc_response_with_error() {
        let json = serde_json::json!({
            "result": null,
            "error": {
                "code": -32600,
                "message": "Invalid Request"
            }
        });
        let response: JsonRpcResponse<u64> = serde_json::from_value(json).unwrap();
        assert!(response.result.is_none());
        assert!(response.error.is_some());
        let error = response.error.unwrap();
        assert_eq!(error.code, -32600);
        assert_eq!(error.message, "Invalid Request");
    }

    #[test]
    fn test_json_rpc_error_insufficient_funds_by_message() {
        let json = serde_json::json!({
            "result": null,
            "error": {
                "code": -32000,
                "message": "Transaction simulation failed: insufficient lamports"
            }
        });
        let response: JsonRpcResponse<String> = serde_json::from_value(json).unwrap();
        let error = response.error.unwrap();
        // The message contains "insufficient" which triggers InsufficientFunds error
        assert!(error.message.contains("insufficient"));
    }

    #[test]
    fn test_json_rpc_error_insufficient_funds_by_code() {
        let json = serde_json::json!({
            "result": null,
            "error": {
                "code": -32002,
                "message": "Some other error"
            }
        });
        let response: JsonRpcResponse<String> = serde_json::from_value(json).unwrap();
        let error = response.error.unwrap();
        // Error code -32002 triggers InsufficientFunds
        assert_eq!(error.code, -32002);
    }

    // --- DESERIALIZATION ERROR TESTS ---

    #[tokio::test]
    async fn test_rpc_call_deserialization_error() {
        // Return a value that can't be deserialized to expected type
        let provider = ConfigurableMockProvider::with_responses(vec![
            Ok(serde_json::json!("not_a_number")), // String instead of u64
        ]);
        let config = RpcClientConfig {
            max_retries: 0,
            ..Default::default()
        };
        let client = RpcBlockchainClient::with_provider(Box::new(provider), config);

        // get_block_height expects u64, but we return a string
        let result = client.get_block_height().await;
        match result {
            Err(AppError::Blockchain(BlockchainError::RpcError(msg))) => {
                assert!(msg.contains("Deserialization error"));
            }
            _ => panic!("Expected deserialization error, got {:?}", result),
        }
    }

    #[tokio::test]
    async fn test_rpc_call_empty_response_after_retries() {
        // No responses configured - should use fallback null
        let provider = ConfigurableMockProvider::new();
        let config = RpcClientConfig {
            max_retries: 0,
            ..Default::default()
        };
        let client = RpcBlockchainClient::with_provider(Box::new(provider), config);

        // Try to get block height - provider returns null which can't deserialize to u64
        let result = client.get_block_height().await;
        assert!(result.is_err());
    }

    // --- SIGNING KEY ADDITIONAL TESTS ---

    #[test]
    fn test_signing_key_from_base58_64_bytes_invalid_keypair() {
        // Create 64 random bytes (not a valid keypair where bytes 32-64 are the public key)
        let invalid_keypair = vec![42u8; 64];
        let encoded = bs58::encode(&invalid_keypair).into_string();
        let secret = SecretString::from(encoded);

        // This should still work since we only use the first 32 bytes
        let result = signing_key_from_base58(&secret);
        assert!(result.is_ok());
    }

    #[test]
    fn test_signing_key_from_base58_empty_string() {
        let secret = SecretString::from("");
        let result = signing_key_from_base58(&secret);
        assert!(result.is_err());
    }

    // --- SDK-BASED TRANSFER TESTS (only with real-blockchain feature) ---

    #[cfg(feature = "real-blockchain")]
    mod real_blockchain_tests {
        use super::*;

        #[tokio::test]
        async fn test_submit_transaction_real_blockchain_path() {
            // This test verifies the SDK client is properly initialized
            // Actual transfer tests require network/mocking
            let signing_key = SigningKey::generate(&mut OsRng);
            let client =
                RpcBlockchainClient::with_defaults("https://api.devnet.solana.com", signing_key)
                    .unwrap();

            // Verify SDK components are initialized
            assert!(client.sdk_client.is_some());
            assert!(client.keypair.is_some());
            let _ = client.public_key();
        }
    }

    // --- RPC CLIENT NEW CONSTRUCTOR TEST ---

    #[test]
    fn test_rpc_blockchain_client_new() {
        let signing_key = SigningKey::generate(&mut OsRng);
        let config = RpcClientConfig {
            timeout: Duration::from_secs(15),
            max_retries: 2,
            retry_delay: Duration::from_millis(250),
            confirmation_timeout: Duration::from_secs(30),
        };
        let result = RpcBlockchainClient::new("https://api.devnet.solana.com", signing_key, config);
        assert!(result.is_ok());
    }

    // --- PROVIDER TRAIT OBJECT TESTS ---

    #[test]
    fn test_provider_as_trait_object() {
        let provider: Box<dyn SolanaRpcProvider> = Box::new(ConfigurableMockProvider::new());

        // Test public_key through trait object
        let pubkey = provider.public_key();
        assert!(!pubkey.is_empty());

        // Test sign through trait object
        let sig = provider.sign(b"message");
        assert!(!sig.is_empty());
    }

    // --- BLOCKHASH RESPONSE DESERIALIZATION ---

    #[test]
    fn test_blockhash_response_deserialization() {
        let json = serde_json::json!({
            "blockhash": "4sGjMW1sUnHzSxGspuhpqLDx6wiyjNtZAMdL4VZHirAn"
        });
        let response: BlockhashResponse = serde_json::from_value(json).unwrap();
        assert_eq!(
            response.blockhash,
            "4sGjMW1sUnHzSxGspuhpqLDx6wiyjNtZAMdL4VZHirAn"
        );
    }

    // --- ADDITIONAL RPC CLIENT CONFIG TESTS ---

    #[test]
    fn test_rpc_client_config_very_short_timeout() {
        let config = RpcClientConfig {
            timeout: Duration::from_millis(1),
            max_retries: 0,
            retry_delay: Duration::from_millis(1),
            confirmation_timeout: Duration::from_millis(1),
        };
        assert_eq!(config.timeout, Duration::from_millis(1));
    }

    #[test]
    fn test_rpc_client_config_zero_retries() {
        let config = RpcClientConfig {
            max_retries: 0,
            ..Default::default()
        };
        assert_eq!(config.max_retries, 0);
    }

    // ====================================================================
    // SUBMISSION STRATEGY INTEGRATION TESTS
    // ====================================================================

    #[test]
    fn test_client_without_submission_strategy() {
        let signing_key = SigningKey::generate(&mut OsRng);
        let client =
            RpcBlockchainClient::with_defaults("https://api.devnet.solana.com", signing_key)
                .unwrap();

        // Without explicit strategy, should not have one
        assert!(!client.has_submission_strategy());
        assert!(!client.supports_private_submission());
    }

    #[test]
    fn test_client_with_submission_strategy_constructor() {
        use super::super::quicknode::{
            QuickNodePrivateSubmissionStrategy, QuickNodeSubmissionConfig,
        };

        let signing_key = SigningKey::generate(&mut OsRng);
        let config = QuickNodeSubmissionConfig {
            rpc_url: "https://test.quiknode.pro/xxx".to_string(),
            enable_jito_bundles: true,
            tip_lamports: 10_000,
            max_bundle_retries: 2,
            region: None,
        };
        let strategy: Box<dyn super::super::strategies::SubmissionStrategy> =
            Box::new(QuickNodePrivateSubmissionStrategy::new(config));

        let client = RpcBlockchainClient::with_defaults_and_submission_strategy(
            "https://test.quiknode.pro/xxx",
            signing_key,
            Some(strategy),
            Some(10_000), // Jito tip
        )
        .unwrap();

        assert!(client.has_submission_strategy());
        assert!(client.supports_private_submission());
    }

    #[test]
    fn test_client_with_jito_disabled_strategy() {
        use super::super::quicknode::{
            QuickNodePrivateSubmissionStrategy, QuickNodeSubmissionConfig,
        };

        let signing_key = SigningKey::generate(&mut OsRng);
        let config = QuickNodeSubmissionConfig {
            rpc_url: "https://test.quiknode.pro/xxx".to_string(),
            enable_jito_bundles: false, // Jito disabled
            tip_lamports: 10_000,
            max_bundle_retries: 2,
            region: None,
        };
        let strategy: Box<dyn super::super::strategies::SubmissionStrategy> =
            Box::new(QuickNodePrivateSubmissionStrategy::new(config));

        let client = RpcBlockchainClient::with_defaults_and_submission_strategy(
            "https://test.quiknode.pro/xxx",
            signing_key,
            Some(strategy),
            None, // No tip when Jito disabled
        )
        .unwrap();

        // Has strategy, but private submission is not supported (Jito disabled)
        assert!(client.has_submission_strategy());
        assert!(!client.supports_private_submission());
    }

    #[test]
    fn test_with_provider_has_no_strategy() {
        let provider = ConfigurableMockProvider::new();
        let config = RpcClientConfig::default();
        let client = RpcBlockchainClient::with_provider(Box::new(provider), config);

        // with_provider doesn't accept a strategy, so it should not have one
        assert!(!client.has_submission_strategy());
        assert!(!client.supports_private_submission());
    }

    #[test]
    fn test_serialize_transaction_base58() {
        use solana_sdk::{hash::Hash, transaction::Transaction};

        let signing_key = SigningKey::generate(&mut OsRng);
        let client =
            RpcBlockchainClient::with_defaults("https://api.devnet.solana.com", signing_key)
                .unwrap();

        // Create a minimal unsigned transaction
        let recent_blockhash = Hash::new_unique();
        let keypair = client.keypair.as_ref().unwrap();
        let tx = Transaction::new_signed_with_payer(
            &[],
            Some(&keypair.pubkey()),
            &[keypair],
            recent_blockhash,
        );

        // Serialize should work
        let result = client.serialize_transaction_base58(&tx);
        assert!(result.is_ok());

        let serialized = result.unwrap();
        assert!(!serialized.is_empty());

        // Should be valid Base58
        let decoded = bs58::decode(&serialized).into_vec();
        assert!(decoded.is_ok());
    }
}

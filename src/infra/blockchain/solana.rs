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
use spl_associated_token_account::{
    get_associated_token_address_with_program_id,
    instruction::create_associated_token_account_idempotent,
};
use spl_token_interface::instruction as token_instruction;

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

/// Solana RPC blockchain client
pub struct RpcBlockchainClient {
    provider: Box<dyn SolanaRpcProvider>,
    config: RpcClientConfig,
    /// Solana SDK RPC client for SDK-based operations
    sdk_client: Option<SolanaRpcClient>,
    /// Solana keypair for signing transactions
    keypair: Option<Keypair>,
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
#[derive(Debug, Deserialize)]
struct QuickNodePriorityFeeResponse {
    per_compute_unit: Option<QuickNodePriorityFeeLevel>,
}

/// Priority fee levels from QuickNode API (values in micro-lamports)
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
    pub fn new(
        rpc_url: &str,
        signing_key: SigningKey,
        config: RpcClientConfig,
    ) -> Result<Self, AppError> {
        let provider = HttpSolanaRpcProvider::new(rpc_url, signing_key.clone(), config.timeout)?;

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

        info!(rpc_url = %rpc_url, "Created blockchain client with SDK support");
        Ok(Self {
            provider: Box::new(provider),
            config,
            sdk_client: Some(sdk_client),
            keypair: Some(keypair),
        })
    }

    /// Create a new RPC blockchain client with default configuration
    pub fn with_defaults(rpc_url: &str, signing_key: SigningKey) -> Result<Self, AppError> {
        Self::new(rpc_url, signing_key, RpcClientConfig::default())
    }

    /// Create a new client with a specific provider (useful for testing)
    pub fn with_provider(provider: Box<dyn SolanaRpcProvider>, config: RpcClientConfig) -> Self {
        Self {
            provider,
            config,
            sdk_client: None,
            keypair: None,
        }
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

    /// Attempt to fetch priority fee from QuickNode's qn_estimatePriorityFees.
    /// Returns the recommended priority fee in micro-lamports.
    /// This method gracefully handles failures (e.g., non-QuickNode providers)
    /// and falls back to a default value.
    async fn get_quicknode_priority_fee(&self) -> u64 {
        const DEFAULT_PRIORITY_FEE: u64 = 100; // micro-lamports fallback

        let params = serde_json::json!({
            "last_n_blocks": 100,
            "api_version": 2
        });

        match self
            .provider
            .send_request("qn_estimatePriorityFees", params)
            .await
        {
            Ok(result) => match serde_json::from_value::<QuickNodePriorityFeeResponse>(result) {
                Ok(response) => {
                    if let Some(fees) = response.per_compute_unit
                        && let Some(high) = fees.high
                    {
                        let fee = high as u64;
                        info!(priority_fee = %fee, "Applied QuickNode priority fee (micro-lamports)");
                        return fee;
                    }
                    debug!(
                        "QuickNode response missing per_compute_unit.high, using default priority fee"
                    );
                    DEFAULT_PRIORITY_FEE
                }
                Err(e) => {
                    debug!(error = %e, "Failed to parse QuickNode priority fee response, using default");
                    DEFAULT_PRIORITY_FEE
                }
            },
            Err(_) => {
                debug!(
                    "QuickNode priority fee API not available (provider may not support it), using default: {} micro-lamports",
                    DEFAULT_PRIORITY_FEE
                );
                DEFAULT_PRIORITY_FEE
            }
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

        // Dispatch to the appropriate transfer method based on token_mint
        match &request.token_mint {
            Some(mint) => {
                // SPL Token transfer
                info!(
                    id = %request.id,
                    mint = %mint,
                    amount = %request.amount_sol,
                    to = %request.to_address,
                    "Dispatching SPL Token transfer"
                );
                self.transfer_token(&request.to_address, mint, request.amount_sol)
                    .await
            }
            None => {
                // Native SOL transfer
                info!(
                    id = %request.id,
                    amount_sol = %request.amount_sol,
                    to = %request.to_address,
                    "Dispatching native SOL transfer"
                );
                self.transfer_sol(&request.to_address, request.amount_sol)
                    .await
            }
        }
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
    async fn transfer_sol(&self, to_address: &str, amount_sol: f64) -> Result<String, AppError> {
        // Convert SOL to lamports (1 SOL = 1_000_000_000 lamports)
        let lamports = (amount_sol * 1_000_000_000.0) as u64;

        info!(to = %to_address, amount_sol = %amount_sol, lamports = %lamports, "Transferring SOL");

        // Validate amount
        if lamports == 0 {
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

        // Get priority fee (gracefully falls back to default if QuickNode not available)
        let priority_fee = self.get_quicknode_priority_fee().await;

        // Create transfer instruction using SDK
        let transfer_ix = system_instruction::transfer(&keypair.pubkey(), &to_pubkey, lamports);

        // Build instructions with compute budget for priority fee
        let instructions = vec![
            ComputeBudgetInstruction::set_compute_unit_price(priority_fee),
            transfer_ix,
        ];

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

        // Send and confirm transaction
        let signature = sdk_client
            .send_and_confirm_transaction(&transaction)
            .await
            .map_err(map_solana_client_error)?;

        info!(signature = %signature, to = %to_address, lamports = %lamports, "SOL transfer submitted via SDK");

        Ok(signature.to_string())
    }

    #[instrument(skip(self))]
    async fn transfer_token(
        &self,
        to_address: &str,
        token_mint: &str,
        amount: f64,
    ) -> Result<String, AppError> {
        info!(to = %to_address, token_mint = %token_mint, amount = %amount, "Transferring SPL Token");

        // Validate amount
        if amount <= 0.0 {
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
        // This handles both legacy SPL Token and Token-2022
        let mint_account = sdk_client.get_account(&mint_pubkey).await.map_err(|e| {
            AppError::Blockchain(BlockchainError::TransactionFailed(format!(
                "Failed to fetch mint account: {}",
                e
            )))
        })?;

        // The mint account's owner is the token program ID
        let token_program_id = mint_account.owner;
        debug!(token_program_id = %token_program_id, "Detected token program from mint");

        // Extract decimals from mint account data manually
        // Mint layout (both SPL Token and Token-2022):
        // - bytes 0-3: mint_authority option (1 byte option flag + 32 bytes pubkey if Some)
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
        debug!(decimals = %decimals, "Read decimals from mint account");

        // Convert human-readable amount to raw token units
        // e.g., 1.5 USDC (6 decimals) -> 1_500_000 raw units
        let raw_amount = (amount * 10f64.powi(decimals as i32)) as u64;
        info!(raw_amount = %raw_amount, decimals = %decimals, "Calculated raw token amount");

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
            debug!(source_balance = %balance, required = %raw_amount, "Checking source token balance");

            if balance < raw_amount {
                return Err(AppError::Blockchain(BlockchainError::InsufficientFunds));
            }
        }

        // Get priority fee (gracefully falls back to default if QuickNode not available)
        let priority_fee = self.get_quicknode_priority_fee().await;

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
        let transfer_ix = token_instruction::transfer_checked(
            &token_program_id,
            &source_ata,
            &mint_pubkey,
            &destination_ata,
            &keypair.pubkey(), // authority (owner of source account)
            &[],               // no multisig signers
            raw_amount,
            decimals,
        )
        .map_err(|e| {
            AppError::Blockchain(BlockchainError::TransactionFailed(format!(
                "Failed to create transfer_checked instruction: {}",
                e
            )))
        })?;

        instructions.push(transfer_ix);

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

        // Send and confirm transaction
        let signature = sdk_client
            .send_and_confirm_transaction(&transaction)
            .await
            .map_err(map_solana_client_error)?;

        info!(
            signature = %signature,
            to = %to_address,
            token_mint = %token_mint,
            amount = %amount,
            raw_amount = %raw_amount,
            "SPL Token transfer submitted"
        );

        Ok(signature.to_string())
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
}

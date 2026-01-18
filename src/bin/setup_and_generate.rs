//! Setup and Generate: Create real on-chain confidential transfer state and generate valid ZK proofs.
//!
//! This script performs the complete workflow on the zk-edge network:
//! 1. Create Token-2022 Mint with ConfidentialTransfer extension
//! 2. Create and configure sender token account for confidential transfers
//! 3. Mint tokens to sender (public balance)
//! 4. Deposit: public -> confidential pending balance
//! 5. Apply pending balance -> available balance
//! 6. Generate ZK proofs using real on-chain state
//! 7. Output TransferRequest JSON for the relayer
//!
//! Usage:
//!   cargo run --bin setup_and_generate

use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use base64::Engine;
use ed25519_dalek::{Signer, SigningKey};
use solana_client::nonblocking::rpc_client::RpcClient;
use solana_commitment_config::CommitmentConfig;
use solana_sdk::{
    signature::Keypair,
    signer::Signer as SolanaSigner,
    transaction::Transaction,
};
use solana_system_interface::instruction as system_instruction;
use solana_zk_sdk::encryption::{
    auth_encryption::{AeCiphertext, AeKey},
    elgamal::{ElGamalCiphertext, ElGamalKeypair},
    pod::auth_encryption::PodAeCiphertext,
};
use spl_associated_token_account::{
    get_associated_token_address_with_program_id,
    instruction::create_associated_token_account_idempotent,
};
use spl_token_2022::{
    extension::{
        confidential_transfer::ConfidentialTransferAccount,
        BaseStateWithExtensions, ExtensionType, StateWithExtensions,
    },
    state::{Account as TokenAccount, Mint},
};
use spl_token_confidential_transfer_proof_extraction::instruction::ProofLocation;
use spl_token_confidential_transfer_proof_generation::transfer::transfer_split_proof_data;
use spl_token_2022::extension::confidential_transfer::instruction::PubkeyValidityProofData;
use std::time::Duration;

use solana_compliance_relayer::domain::types::{SubmitTransferRequest, TransferType};

// Network configuration
const RPC_URL: &str = "https://zk-edge.surfnet.dev:8899";

// Token configuration
const TOKEN_DECIMALS: u8 = 9;
const MINT_AMOUNT: u64 = 100_000_000_000; // 100 tokens
const TRANSFER_AMOUNT: u64 = 1_000_000_000; // 1 token

fn print_separator(title: &str) {
    println!("{}", "=".repeat(70));
    println!("  {}", title);
    println!("{}", "=".repeat(70));
}

/// Helper to send a transaction with retry logic for blockhash expiry
async fn send_with_retry<F>(
    rpc_client: &RpcClient,
    build_tx: F,
    max_retries: u32,
) -> Result<solana_sdk::signature::Signature, Box<dyn std::error::Error>>
where
    F: Fn(solana_sdk::hash::Hash) -> Transaction,
{
    use solana_client::rpc_config::RpcSendTransactionConfig;
    
    for attempt in 0..max_retries {
        // Wait before getting blockhash to ensure network sync (especially for remote RPC)
        if attempt > 0 {
            println!("    Retrying ({}/{})...", attempt + 1, max_retries);
            tokio::time::sleep(Duration::from_secs(3)).await;
        }
        
        // Get fresh blockhash for each attempt
        let blockhash = rpc_client.get_latest_blockhash().await?;
        
        let tx = build_tx(blockhash);
        
        // Skip preflight to avoid blockhash mismatch during simulation
        // The zk-edge testnet has sync issues between validators
        let config = RpcSendTransactionConfig {
            skip_preflight: true,
            ..Default::default()
        };
        
        match rpc_client.send_transaction_with_config(&tx, config).await {
            Ok(sig) => {
                // Wait for confirmation manually
                println!("    Transaction sent: {}", sig);
                for _ in 0..30 {
                    tokio::time::sleep(Duration::from_secs(1)).await;
                    match rpc_client.get_signature_statuses(&[sig]).await {
                        Ok(statuses) => {
                            if let Some(Some(status)) = statuses.value.first() {
                                if status.err.is_none() {
                                    return Ok(sig);
                                } else {
                                    // Transaction failed
                                    return Err(format!("Transaction failed: {:?}", status.err).into());
                                }
                            }
                        }
                        Err(_) => continue,
                    }
                }
                // If we get here, transaction may have expired
                println!("    Transaction confirmation timeout, retrying...");
                continue;
            }
            Err(e) => {
                // Use debug format to capture full error structure including nested enums
                let error_str = format!("{:?}", e);
                let is_blockhash_error = error_str.contains("BlockhashNotFound") 
                    || error_str.contains("blockhash")
                    || error_str.to_lowercase().contains("blockhash not found");
                
                if is_blockhash_error && attempt < max_retries - 1 {
                    println!("    Blockhash expired, will retry...");
                    continue;
                }
                return Err(e.into());
            }
        }
    }
    Err("Max retries exceeded".into())
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    print_separator("Confidential Transfer Setup & Generate");
    println!("  Network: {}", RPC_URL);
    println!();

    // Create RPC client
    let rpc_client = RpcClient::new_with_timeout_and_commitment(
        RPC_URL.to_string(),
        Duration::from_secs(60),
        CommitmentConfig::confirmed(),
    );

    // Step 1: Create or load keypairs
    println!("Step 1: Setting up keypairs...");
    
    // Main authority keypair (pays for everything, owns the mint)
    let authority = Keypair::new();
    println!("  Authority pubkey: {}", authority.pubkey());
    
    // Request airdrop for transaction fees
    println!("  Requesting airdrop...");
    match rpc_client.request_airdrop(&authority.pubkey(), 2_000_000_000).await {
        Ok(sig) => {
            println!("  Airdrop signature: {}", sig);
            // Wait for airdrop confirmation
            tokio::time::sleep(Duration::from_secs(3)).await;
            let balance = rpc_client.get_balance(&authority.pubkey()).await?;
            println!("  Balance after airdrop: {} lamports", balance);
        }
        Err(e) => {
            println!("  Airdrop failed (may already have funds): {}", e);
        }
    }
    
    // ElGamal keypair for source account encryption
    let source_elgamal_keypair = ElGamalKeypair::new_rand();
    let source_aes_key = AeKey::new_rand();
    
    // ElGamal keypair for destination account encryption
    let dest_elgamal_keypair = ElGamalKeypair::new_rand();
    let dest_aes_key = AeKey::new_rand();
    
    // Destination wallet (different from source)
    let destination_wallet = Keypair::new();
    
    println!("  Source ElGamal pubkey generated");
    println!("  Destination wallet: {}", destination_wallet.pubkey());
    println!();

    // Step 2: Create Token-2022 Mint with ConfidentialTransfer extension
    println!("Step 2: Creating Token-2022 Mint with ConfidentialTransfer extension...");
    
    let mint_keypair = Keypair::new();
    let mint_pubkey = mint_keypair.pubkey();
    
    // Calculate space needed for mint with extensions
    let extensions = [ExtensionType::ConfidentialTransferMint];
    let mint_space = ExtensionType::try_calculate_account_len::<Mint>(&extensions)?;
    let mint_rent = rpc_client.get_minimum_balance_for_rent_exemption(mint_space).await?;
    
    println!("  Mint pubkey: {}", mint_pubkey);
    println!("  Mint space: {} bytes", mint_space);
    println!("  Mint rent: {} lamports", mint_rent);
    
    // Build mint creation instructions
    let create_mint_account_ix = system_instruction::create_account(
        &authority.pubkey(),
        &mint_pubkey,
        mint_rent,
        mint_space as u64,
        &spl_token_2022::id(),
    );
    
    // Initialize ConfidentialTransferMint extension using the new interface
    // Note: Using spl_token_2022::extension::confidential_transfer::instruction
    let init_ct_mint_ix = spl_token_2022::extension::confidential_transfer::instruction::initialize_mint(
        &spl_token_2022::id(),
        &mint_pubkey,
        Some(authority.pubkey()), // CT authority
        true,                     // auto_approve_new_accounts
        None,                     // auditor (no auditor)
    )?;
    
    // Initialize the mint itself
    let init_mint_ix = spl_token_2022::instruction::initialize_mint(
        &spl_token_2022::id(),
        &mint_pubkey,
        &authority.pubkey(),
        Some(&authority.pubkey()),
        TOKEN_DECIMALS,
    )?;
    
    // Clone instructions for retry closure
    let create_mint_instructions = vec![create_mint_account_ix, init_ct_mint_ix, init_mint_ix];
    let sig = send_with_retry(
        &rpc_client,
        |blockhash| {
            Transaction::new_signed_with_payer(
                &create_mint_instructions,
                Some(&authority.pubkey()),
                &[&authority, &mint_keypair],
                blockhash,
            )
        },
        5, // max retries
    ).await?;
    println!("  Mint created! Signature: {}", sig);
    println!();

    // Step 3: Create Source Token Account and Configure for Confidential Transfers
    println!("Step 3: Creating source token account with confidential transfers...");
    
    let source_ata = get_associated_token_address_with_program_id(
        &authority.pubkey(),
        &mint_pubkey,
        &spl_token_2022::id(),
    );
    println!("  Source ATA: {}", source_ata);
    
    // Create the ATA
    let create_source_ata_ix = create_associated_token_account_idempotent(
        &authority.pubkey(),
        &authority.pubkey(),
        &mint_pubkey,
        &spl_token_2022::id(),
    );
    
    let recent_blockhash = rpc_client.get_latest_blockhash().await?;
    let create_ata_tx = Transaction::new_signed_with_payer(
        &[create_source_ata_ix],
        Some(&authority.pubkey()),
        &[&authority],
        recent_blockhash,
    );
    
    let sig = rpc_client.send_and_confirm_transaction(&create_ata_tx).await?;
    println!("  ATA created! Signature: {}", sig);
    
    // Reallocate account to add space for ConfidentialTransferAccount extension
    println!("  Reallocating account for confidential transfer extension...");
    
    let reallocate_ix = spl_token_2022::instruction::reallocate(
        &spl_token_2022::id(),
        &source_ata,
        &authority.pubkey(),
        &authority.pubkey(),
        &[],
        &[ExtensionType::ConfidentialTransferAccount],
    )?;
    
    let recent_blockhash = rpc_client.get_latest_blockhash().await?;
    let reallocate_tx = Transaction::new_signed_with_payer(
        &[reallocate_ix],
        Some(&authority.pubkey()),
        &[&authority],
        recent_blockhash,
    );
    
    let sig = rpc_client.send_and_confirm_transaction(&reallocate_tx).await?;
    println!("  Reallocated! Signature: {}", sig);
    
    // Configure the account for confidential transfers
    println!("  Configuring account for confidential transfers...");
    
    // Initial decryptable balance = 0
    let initial_balance: AeCiphertext = source_aes_key.encrypt(0u64);
    let initial_balance_pod = PodAeCiphertext::from(initial_balance);
    
    // Generate pubkey validity proof data
    let pubkey_validity_proof_data = PubkeyValidityProofData::new(&source_elgamal_keypair)?;
    
    // Use configure_account which returns Vec<Instruction> with all needed instructions
    let configure_account_ixs = spl_token_2022::extension::confidential_transfer::instruction::configure_account(
        &spl_token_2022::id(),
        &source_ata,
        &mint_pubkey,
        &initial_balance_pod,
        u64::MAX, // maximum pending balance credit counter
        &authority.pubkey(),
        &[],
        ProofLocation::InstructionOffset(
            1.try_into().unwrap(),
            &pubkey_validity_proof_data,
        ),
    )?;
    
    let recent_blockhash = rpc_client.get_latest_blockhash().await?;
    let configure_tx = Transaction::new_signed_with_payer(
        &configure_account_ixs,
        Some(&authority.pubkey()),
        &[&authority],
        recent_blockhash,
    );
    
    let sig = rpc_client.send_and_confirm_transaction(&configure_tx).await?;
    println!("  Account configured for CT! Signature: {}", sig);
    println!();

    // Step 4: Mint tokens to source account (public balance)
    println!("Step 4: Minting {} tokens to source account...", MINT_AMOUNT / 1_000_000_000);
    
    let mint_to_ix = spl_token_2022::instruction::mint_to(
        &spl_token_2022::id(),
        &mint_pubkey,
        &source_ata,
        &authority.pubkey(),
        &[],
        MINT_AMOUNT,
    )?;
    
    let recent_blockhash = rpc_client.get_latest_blockhash().await?;
    let mint_tx = Transaction::new_signed_with_payer(
        &[mint_to_ix],
        Some(&authority.pubkey()),
        &[&authority],
        recent_blockhash,
    );
    
    let sig = rpc_client.send_and_confirm_transaction(&mint_tx).await?;
    println!("  Minted! Signature: {}", sig);
    println!();

    // Step 5: Deposit tokens from public to confidential pending balance
    println!("Step 5: Depositing {} tokens to confidential pending balance...", MINT_AMOUNT / 1_000_000_000);
    
    let deposit_ix = spl_token_2022::extension::confidential_transfer::instruction::deposit(
        &spl_token_2022::id(),
        &source_ata,
        &mint_pubkey,
        MINT_AMOUNT,
        TOKEN_DECIMALS,
        &authority.pubkey(),
        &[],
    )?;
    
    let recent_blockhash = rpc_client.get_latest_blockhash().await?;
    let deposit_tx = Transaction::new_signed_with_payer(
        &[deposit_ix],
        Some(&authority.pubkey()),
        &[&authority],
        recent_blockhash,
    );
    
    let sig = rpc_client.send_and_confirm_transaction(&deposit_tx).await?;
    println!("  Deposited! Signature: {}", sig);
    println!();

    // Step 6: Apply pending balance to available balance
    println!("Step 6: Applying pending balance to available balance...");
    
    // Wait a moment for the deposit to be processed
    tokio::time::sleep(Duration::from_secs(3)).await;
    
    // Fetch account to get current state
    let account_data = rpc_client.get_account(&source_ata).await?;
    let account_state = StateWithExtensions::<TokenAccount>::unpack(&account_data.data)?;
    let ct_extension = account_state.get_extension::<ConfidentialTransferAccount>()?;
    
    // Create new encrypted balance
    let new_decryptable_available_balance: AeCiphertext = source_aes_key.encrypt(MINT_AMOUNT);
    let new_decryptable_pod = PodAeCiphertext::from(new_decryptable_available_balance);
    
    let apply_pending_ix = spl_token_2022::extension::confidential_transfer::instruction::apply_pending_balance(
        &spl_token_2022::id(),
        &source_ata,
        ct_extension.pending_balance_credit_counter.into(),
        &new_decryptable_pod,
        &authority.pubkey(),
        &[],
    )?;
    
    let recent_blockhash = rpc_client.get_latest_blockhash().await?;
    let apply_tx = Transaction::new_signed_with_payer(
        &[apply_pending_ix],
        Some(&authority.pubkey()),
        &[&authority],
        recent_blockhash,
    );
    
    let sig = rpc_client.send_and_confirm_transaction(&apply_tx).await?;
    println!("  Applied! Signature: {}", sig);
    println!();

    // Step 7: Create destination account with confidential transfers configured
    println!("Step 7: Creating destination token account with confidential transfers...");
    
    // Fund destination wallet for transaction fees
    let fund_dest_ix = system_instruction::transfer(
        &authority.pubkey(),
        &destination_wallet.pubkey(),
        100_000_000,
    );
    
    let recent_blockhash = rpc_client.get_latest_blockhash().await?;
    let fund_tx = Transaction::new_signed_with_payer(
        &[fund_dest_ix],
        Some(&authority.pubkey()),
        &[&authority],
        recent_blockhash,
    );
    let _ = rpc_client.send_and_confirm_transaction(&fund_tx).await;
    
    let dest_ata = get_associated_token_address_with_program_id(
        &destination_wallet.pubkey(),
        &mint_pubkey,
        &spl_token_2022::id(),
    );
    println!("  Destination ATA: {}", dest_ata);
    
    // Create destination ATA
    let create_dest_ata_ix = create_associated_token_account_idempotent(
        &authority.pubkey(),
        &destination_wallet.pubkey(),
        &mint_pubkey,
        &spl_token_2022::id(),
    );
    
    let recent_blockhash = rpc_client.get_latest_blockhash().await?;
    let create_dest_tx = Transaction::new_signed_with_payer(
        &[create_dest_ata_ix],
        Some(&authority.pubkey()),
        &[&authority],
        recent_blockhash,
    );
    let sig = rpc_client.send_and_confirm_transaction(&create_dest_tx).await?;
    println!("  Destination ATA created! Signature: {}", sig);
    
    // Reallocate destination account to add space for ConfidentialTransferAccount extension
    println!("  Reallocating destination account for confidential transfer extension...");
    
    let reallocate_dest_ix = spl_token_2022::instruction::reallocate(
        &spl_token_2022::id(),
        &dest_ata,
        &authority.pubkey(), // payer
        &destination_wallet.pubkey(), // owner
        &[],
        &[ExtensionType::ConfidentialTransferAccount],
    )?;
    
    let recent_blockhash = rpc_client.get_latest_blockhash().await?;
    let reallocate_dest_tx = Transaction::new_signed_with_payer(
        &[reallocate_dest_ix],
        Some(&authority.pubkey()),
        &[&authority, &destination_wallet], // owner must sign
        recent_blockhash,
    );
    
    let sig = rpc_client.send_and_confirm_transaction(&reallocate_dest_tx).await?;
    println!("  Destination reallocated! Signature: {}", sig);
    
    // Configure destination for confidential transfers
    let dest_initial_balance = PodAeCiphertext::from(dest_aes_key.encrypt(0u64));
    let dest_pubkey_validity_proof_data = PubkeyValidityProofData::new(&dest_elgamal_keypair)?;
    
    // Use configure_account which returns Vec<Instruction> with all needed instructions
    let configure_dest_ixs = spl_token_2022::extension::confidential_transfer::instruction::configure_account(
        &spl_token_2022::id(),
        &dest_ata,
        &mint_pubkey,
        &dest_initial_balance,
        u64::MAX,
        &destination_wallet.pubkey(),
        &[],
        ProofLocation::InstructionOffset(
            1.try_into().unwrap(),
            &dest_pubkey_validity_proof_data,
        ),
    )?;
    
    let recent_blockhash = rpc_client.get_latest_blockhash().await?;
    let configure_dest_tx = Transaction::new_signed_with_payer(
        &configure_dest_ixs,
        Some(&destination_wallet.pubkey()),
        &[&destination_wallet],
        recent_blockhash,
    );
    let sig = rpc_client.send_and_confirm_transaction(&configure_dest_tx).await?;
    println!("  Destination configured for CT! Signature: {}", sig);
    println!();

    // Step 8: Generate ZK proofs for confidential transfer using REAL on-chain state
    println!("Step 8: Generating ZK proofs from real on-chain state...");
    println!("  Transfer amount: {} tokens", TRANSFER_AMOUNT / 1_000_000_000);
    
    // Wait for state to settle
    tokio::time::sleep(Duration::from_secs(2)).await;
    
    // Fetch current account state
    let account_data = rpc_client.get_account(&source_ata).await?;
    let account_state = StateWithExtensions::<TokenAccount>::unpack(&account_data.data)?;
    let ct_extension = account_state.get_extension::<ConfidentialTransferAccount>()?;
    
    // Get current available balance (encrypted with source ElGamal key)
    let current_available_balance: ElGamalCiphertext = ct_extension
        .available_balance
        .try_into()
        .map_err(|_| "Failed to parse available balance")?;
    
    // Current decryptable balance (encrypted with AES key)
    let current_decryptable_balance: AeCiphertext = ct_extension
        .decryptable_available_balance
        .try_into()
        .map_err(|_| "Failed to parse decryptable balance")?;
    
    println!("  Current available balance fetched from chain");
    
    // Generate the ZK proofs for the transfer
    println!("  Generating ZK proofs (this may take a moment)...");
    
    let proof_data = transfer_split_proof_data(
        &current_available_balance,
        &current_decryptable_balance,
        TRANSFER_AMOUNT,
        &source_elgamal_keypair,
        &source_aes_key,
        dest_elgamal_keypair.pubkey(),
        None,
    )?;
    
    println!("  ZK proofs generated successfully!");
    
    // Encode proofs to base64
    let equality_proof_bytes = bytemuck::bytes_of(&proof_data.equality_proof_data);
    let equality_proof_base64 = BASE64_STANDARD.encode(equality_proof_bytes);
    
    let validity_proof_bytes = bytemuck::bytes_of(&proof_data.ciphertext_validity_proof_data_with_ciphertext.proof_data);
    let ciphertext_validity_proof_base64 = BASE64_STANDARD.encode(validity_proof_bytes);
    
    let range_proof_bytes = bytemuck::bytes_of(&proof_data.range_proof_data);
    let range_proof_base64 = BASE64_STANDARD.encode(range_proof_bytes);
    
    // Calculate new decryptable balance after transfer
    let new_source_balance = MINT_AMOUNT - TRANSFER_AMOUNT;
    let new_decryptable_balance: AeCiphertext = source_aes_key.encrypt(new_source_balance);
    let new_decryptable_balance_base64 = BASE64_STANDARD.encode(new_decryptable_balance.to_bytes());
    
    println!();
    println!("Proof sizes:");
    println!("  Equality proof:            {} bytes", equality_proof_bytes.len());
    println!("  Ciphertext validity proof: {} bytes", validity_proof_bytes.len());
    println!("  Range proof:               {} bytes", range_proof_bytes.len());
    println!("  New decryptable balance:   {} bytes", new_decryptable_balance.to_bytes().len());
    println!();

    // Step 9: Generate the transfer request JSON
    println!("Step 9: Generating TransferRequest for relayer...");
    
    // Create Ed25519 signing key from the authority keypair
    let signing_key = SigningKey::from_bytes(&authority.to_bytes()[..32].try_into()?);
    let from_pubkey = authority.pubkey();
    let to_pubkey = destination_wallet.pubkey();
    
    // Create signing message
    let message = format!(
        "{}:{}:confidential:{}",
        from_pubkey, to_pubkey, mint_pubkey
    );
    println!("  Signing message: \"{}\"", message);
    
    let signature = signing_key.sign(message.as_bytes());
    let signature_bs58 = bs58::encode(signature.to_bytes()).into_string();
    
    let transfer_details = TransferType::Confidential {
        new_decryptable_available_balance: new_decryptable_balance_base64,
        equality_proof: equality_proof_base64,
        ciphertext_validity_proof: ciphertext_validity_proof_base64,
        range_proof: range_proof_base64,
    };
    
    let request = SubmitTransferRequest {
        from_address: from_pubkey.to_string(),
        to_address: to_pubkey.to_string(),
        transfer_details,
        token_mint: Some(mint_pubkey.to_string()),
        signature: signature_bs58,
    };
    
    let json_body = serde_json::to_string_pretty(&request)?;
    
    println!();
    print_separator("SETUP COMPLETE!");
    println!();
    println!("Key Information:");
    println!("  Mint:              {}", mint_pubkey);
    println!("  Source ATA:        {}", source_ata);
    println!("  Destination ATA:   {}", dest_ata);
    println!("  Authority:         {}", authority.pubkey());
    println!("  Destination:       {}", destination_wallet.pubkey());
    println!();
    println!("Authority Private Key (save for future use):");
    println!("  {:?}", authority.to_bytes());
    println!();
    print_separator("GENERATED CURL COMMAND");
    println!();
    
    let curl_cmd = format!(
        "curl -X POST 'http://localhost:3000/transfer-requests' \\\n  -H 'Content-Type: application/json' \\\n  -d '{}'",
        json_body
    );
    
    println!("{}", curl_cmd);
    println!();
    
    Ok(())
}

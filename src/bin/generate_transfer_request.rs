//! CLI utility to generate valid transfer requests with proper Ed25519 signatures.
//!
//! Usage:
//!   cargo run --bin generate_transfer_request              # Public transfer
//!   cargo run --bin generate_transfer_request -- --confidential  # Confidential transfer with ZK proofs

use anyhow::{Context, Result, bail};
use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use ed25519_dalek::{Signer, SigningKey};
use rand::TryRng;
use solana_compliance_relayer::domain::types::{SubmitTransferRequest, TransferType};
use solana_sdk::pubkey::Pubkey;

// ZK cryptography imports for confidential transfers
use solana_zk_sdk::encryption::{
    auth_encryption::{AeCiphertext, AeKey},
    elgamal::{ElGamalCiphertext, ElGamalKeypair},
};
use spl_token_confidential_transfer_proof_generation::transfer::transfer_split_proof_data;

const DEMO_SIGNER_PRIVATE_KEY_ENV: &str = "DEMO_SIGNER_PRIVATE_KEY_B58";

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let is_confidential = args.iter().any(|arg| arg == "--confidential");

    // 1. Generate an ephemeral Ed25519 signer for this run. For reproducible
    // local demos only, DEMO_SIGNER_PRIVATE_KEY_B58 may contain a throwaway
    // 32-byte seed or 64-byte Solana keypair. Never use production key material.
    let (signing_key, signer_source) = load_demo_signing_key()?;
    let verify_key = signing_key.verifying_key();
    // Convert to Solana Pubkeys for consistent display
    let from_pubkey = Pubkey::from(verify_key.to_bytes());
    // Use a random destination address
    let to_pubkey = Pubkey::new_unique();

    println!("Demo Signer:");
    println!("   Public Key (from_address): {}", from_pubkey);
    println!("   Source: {}", signer_source);
    println!("\n--------------------------------------------------\n");

    // 2. Prepare request data based on transfer type
    let (transfer_details, token_mint, amount_part): (TransferType, Option<String>, String) =
        if is_confidential {
            generate_confidential_transfer()?
        } else {
            let amount = 1_000_000_000u64; // 1 SOL
            (TransferType::Public { amount }, None, amount.to_string())
        };

    // Generate a unique nonce (UUID v7 recommended for time-ordered uniqueness; v4 also valid)
    let nonce = uuid::Uuid::now_v7().to_string();

    // Construct the message exactly as the server expects for signing:
    // "{from_address}:{to_address}:{amount|confidential}:{token_mint|SOL}:{nonce}"
    let mint_str = token_mint.as_deref().unwrap_or("SOL");
    let message = format!(
        "{}:{}:{}:{}:{}",
        from_pubkey, to_pubkey, amount_part, mint_str, nonce
    );

    println!("Nonce: \"{}\"", nonce);
    println!("Signing Message: \"{}\"", message);

    // 3. Sign the message
    let signature = signing_key.sign(message.as_bytes());
    let signature_bs58 = bs58::encode(signature.to_bytes()).into_string();

    // 4. Construct the Request Object
    let request = SubmitTransferRequest {
        from_address: from_pubkey.to_string(),
        to_address: to_pubkey.to_string(),
        transfer_details,
        token_mint,
        signature: signature_bs58.clone(),
        nonce: nonce.clone(),
    };

    // 5. Generate the CURL command (with optional Idempotency-Key header)
    let json_body = serde_json::to_string_pretty(&request)
        .context("failed to serialize transfer request JSON")?;
    let curl_cmd = format!(
        "curl -X POST 'http://localhost:3000/transfer-requests' \\\n  -H 'Content-Type: application/json' \\\n  -H 'Idempotency-Key: {}' \\\n  -d '{}'",
        nonce, json_body
    );

    println!("\nGenerated curl command:\n");
    println!("{}", curl_cmd);

    Ok(())
}

fn load_demo_signing_key() -> Result<(SigningKey, &'static str)> {
    match std::env::var(DEMO_SIGNER_PRIVATE_KEY_ENV) {
        Ok(encoded) => parse_demo_signing_key(&encoded).map(|signing_key| {
            (
                signing_key,
                "DEMO_SIGNER_PRIVATE_KEY_B58 (value not printed)",
            )
        }),
        Err(std::env::VarError::NotPresent) => {
            let mut seed = [0_u8; 32];
            let mut rng = rand::rngs::SysRng;
            rng.try_fill_bytes(&mut seed)
                .context("failed to read OS randomness for demo signer")?;
            Ok((
                SigningKey::from_bytes(&seed),
                "ephemeral Ed25519 key for this run",
            ))
        }
        Err(std::env::VarError::NotUnicode(_)) => {
            bail!("{DEMO_SIGNER_PRIVATE_KEY_ENV} must contain valid Unicode base58 text")
        }
    }
}

fn parse_demo_signing_key(encoded: &str) -> Result<SigningKey> {
    if encoded.is_empty() {
        bail!("{DEMO_SIGNER_PRIVATE_KEY_ENV} is set but empty");
    }

    if encoded != encoded.trim() {
        bail!("{DEMO_SIGNER_PRIVATE_KEY_ENV} must not contain leading or trailing whitespace");
    }

    let key_bytes = bs58::decode(encoded).into_vec().with_context(|| {
        format!("{DEMO_SIGNER_PRIVATE_KEY_ENV} must be valid base58-encoded key material")
    })?;

    let key_length = key_bytes.len();
    if key_length != 32 && key_length != 64 {
        bail!(
            "{DEMO_SIGNER_PRIVATE_KEY_ENV} must decode to 32 bytes or 64 bytes, got {key_length}"
        );
    }

    let mut seed = [0_u8; 32];
    seed.copy_from_slice(&key_bytes[..32]);
    let signing_key = SigningKey::from_bytes(&seed);

    if key_length == 64 {
        let mut public_key = [0_u8; 32];
        public_key.copy_from_slice(&key_bytes[32..]);

        if signing_key.verifying_key().to_bytes() != public_key {
            bail!(
                "{DEMO_SIGNER_PRIVATE_KEY_ENV} contains a 64-byte Solana keypair whose public key half does not match the private seed"
            );
        }
    }

    Ok(signing_key)
}

/// Generate a confidential transfer with real ZK proofs.
///
/// This simulates a source account with a known balance, generates all required
/// ElGamal/AES cryptographic keys, and produces valid ZK proofs for the transfer.
fn generate_confidential_transfer() -> Result<(TransferType, Option<String>, String)> {
    println!("Generating CONFIDENTIAL transfer with real ZK proofs...\n");

    // Simulated account state
    const INITIAL_BALANCE: u64 = 100_000_000_000; // 10 SOL in lamports
    const TRANSFER_AMOUNT: u64 = 1_000_000_000; // 1 SOL in lamports

    // Generate cryptographic keys
    let source_elgamal_keypair = ElGamalKeypair::new_rand();
    let destination_elgamal_keypair = ElGamalKeypair::new_rand();
    let aes_key = AeKey::new_rand();

    println!("Simulated account state:");
    println!(
        "Initial balance: {} lamports ({} SOL)",
        INITIAL_BALANCE,
        INITIAL_BALANCE / 1_000_000_000
    );
    println!(
        "Transfer amount: {} lamports ({} SOL)",
        TRANSFER_AMOUNT,
        TRANSFER_AMOUNT / 1_000_000_000
    );
    println!(
        "Remaining balance: {} lamports ({} SOL)",
        INITIAL_BALANCE - TRANSFER_AMOUNT,
        (INITIAL_BALANCE - TRANSFER_AMOUNT) / 1_000_000_000
    );

    // Encrypt the current balance as ElGamal ciphertext (what's stored on-chain)
    let current_available_balance: ElGamalCiphertext =
        source_elgamal_keypair.pubkey().encrypt(INITIAL_BALANCE);

    // Encrypt the current balance as AeCiphertext (decryptable by owner)
    let current_decryptable_balance: AeCiphertext = aes_key.encrypt(INITIAL_BALANCE);

    // Generate the ZK proofs for the transfer
    println!("\nGenerating ZK proofs (this may take a moment)...");

    let proof_data = transfer_split_proof_data(
        &current_available_balance,
        &current_decryptable_balance,
        TRANSFER_AMOUNT,
        &source_elgamal_keypair,
        &aes_key,
        destination_elgamal_keypair.pubkey(),
        None, // No auditor
    )
    .map_err(|err| anyhow::anyhow!("failed to generate ZK proofs: {err:?}"))?;

    println!("ZK proofs generated successfully!");

    // Extract and encode the proofs
    // The equality proof data
    let equality_proof_bytes = bytemuck::bytes_of(&proof_data.equality_proof_data);
    let equality_proof_base64 = BASE64_STANDARD.encode(equality_proof_bytes);

    // The ciphertext validity proof (includes the grouped ciphertext)
    let validity_proof = &proof_data.ciphertext_validity_proof_data_with_ciphertext;
    let validity_proof_bytes = bytemuck::bytes_of(&validity_proof.proof_data);
    let ciphertext_validity_proof_base64 = BASE64_STANDARD.encode(validity_proof_bytes);

    // The range proof
    let range_proof_bytes = bytemuck::bytes_of(&proof_data.range_proof_data);
    let range_proof_base64 = BASE64_STANDARD.encode(range_proof_bytes);

    // Calculate new decryptable balance after transfer
    let new_balance = INITIAL_BALANCE - TRANSFER_AMOUNT;
    let new_decryptable_balance: AeCiphertext = aes_key.encrypt(new_balance);
    let new_decryptable_balance_bytes = new_decryptable_balance.to_bytes();
    let new_decryptable_balance_base64 = BASE64_STANDARD.encode(new_decryptable_balance_bytes);

    // Print proof sizes for verification
    println!("\nProof sizes:");
    println!(
        "Equality proof:            {} bytes",
        equality_proof_bytes.len()
    );
    println!(
        "Ciphertext validity proof: {} bytes",
        validity_proof_bytes.len()
    );
    println!(
        "Range proof:               {} bytes",
        range_proof_bytes.len()
    );
    println!("New decryptable balance:   36 bytes");

    // Use a random token mint for the confidential transfer
    let token_mint = Pubkey::new_unique().to_string();

    let transfer_details = TransferType::Confidential {
        new_decryptable_available_balance: new_decryptable_balance_base64,
        equality_proof: equality_proof_base64,
        ciphertext_validity_proof: ciphertext_validity_proof_base64,
        range_proof: range_proof_base64,
    };

    Ok((
        transfer_details,
        Some(token_mint),
        "confidential".to_string(),
    ))
}

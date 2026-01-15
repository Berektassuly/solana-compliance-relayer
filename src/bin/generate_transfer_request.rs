use ed25519_dalek::{Signer, SigningKey};
use rand::rngs::OsRng;
use solana_compliance_relayer::domain::types::{SubmitTransferRequest, TransferType};
use solana_sdk::pubkey::Pubkey;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let is_confidential = args.iter().any(|arg| arg == "--confidential");

    // 1. Generate a random Ed25519 keypair
    let mut csprng = OsRng;
    let signing_key = SigningKey::generate(&mut csprng);
    let verify_key = signing_key.verifying_key();

    // Convert to Solana Pubkeys for consistent display
    let from_pubkey = Pubkey::try_from(verify_key.to_bytes()).unwrap();
    // Use a random destination address
    let to_pubkey = Pubkey::new_unique();

    println!(" Generated Keypair:");
    println!(" Public Key (from_address): {}", from_pubkey);
    // Note: In a real app never print private keys. This is a dev tool.
    println!(" Private Key (keep safe):  {:?}", signing_key.to_bytes());
    println!("\n--------------------------------------------------\n");

    // 2. Prepare request data
    let amount = 1_000_000_000; // 1 SOL
    let token_mint: Option<String> = if is_confidential {
        Some(Pubkey::new_unique().to_string())
    } else {
        None
    };

    let transfer_details = if is_confidential {
        TransferType::Confidential {
            new_decryptable_available_balance: "mock_balance_ciphertext".to_string(),
            equality_proof: "mock_equality_proof".to_string(),
            ciphertext_validity_proof: "mock_validity_proof".to_string(),
            range_proof: "mock_range_proof".to_string(),
        }
    } else {
        TransferType::Public { amount }
    };

    // Construct the message exactly as the server expects for signing:
    // "{from_address}:{to_address}:{amount|confidential}:{token_mint|SOL}"
    let amount_part = if is_confidential {
        "confidential".to_string()
    } else {
        amount.to_string()
    };

    let mint_str = token_mint.as_deref().unwrap_or("SOL");

    let message = format!("{}:{}:{}:{}", from_pubkey, to_pubkey, amount_part, mint_str);

    println!("Signing Message: \"{}\"", message);

    // 3. Sign the message
    let signature = signing_key.sign(message.as_bytes());
    let signature_bs58 = bs58::encode(signature.to_bytes()).into_string();

    // 4. Construct the Request Object (merely for structure visualization if needed)
    let request = SubmitTransferRequest {
        from_address: from_pubkey.to_string(),
        to_address: to_pubkey.to_string(),
        transfer_details,
        token_mint,
        signature: signature_bs58.clone(),
    };

    // 5. Generate the CURL command
    // Note: We manually construct the JSON to ensure the enum variant `type` field is clear
    // although serde_json would handle it if we used the struct.
    let json_body = serde_json::to_string_pretty(&request).unwrap();

    // Escape single quotes for shell safety if needed (simple approach here)
    let curl_cmd = format!(
        "curl -X POST 'http://localhost:3000/transfer-requests' \\\n  -H 'Content-Type: application/json' \\\n  -d '{}'",
        json_body
    );

    println!("\nGenerated curl command:\n");
    println!("{}", curl_cmd);
}

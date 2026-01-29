//! Blockchain client implementations.
//!
//! This module provides blockchain interaction abstractions with provider-specific
//! strategy implementations for Helius, QuickNode, and standard Solana RPC.

pub mod helius;
pub mod quicknode;
pub mod solana;
pub mod strategies;

// Re-export main types
pub use solana::{RpcBlockchainClient, RpcClientConfig, signing_key_from_base58};

// Re-export strategy types
pub use strategies::{FeeStrategy, RpcProviderType, SubmissionStrategy};

// Re-export Helius-specific types
pub use helius::{HeliusDasClient, HeliusFeeStrategy, SANCTIONED_COLLECTIONS};

// Re-export QuickNode-specific types
pub use quicknode::{
    QuickNodePrivateSubmissionStrategy, QuickNodeSubmissionConfig, QuickNodeTokenApiClient,
    StandardSubmissionStrategy, TokenActivityInfo,
};

// ============================================================================
// JITO TIP ACCOUNTS
// ============================================================================

/// Official Jito Tip Account addresses (hardcoded to avoid extra RPC call).
///
/// These are the 8 accounts that accept tips for Jito block builders.
/// A tip transfer to one of these accounts is REQUIRED for bundle acceptance.
///
/// Source: <https://www.quicknode.com/docs/solana/getTipAccounts>
///
/// Best practice: Select one at random to reduce contention on specific addresses.
pub const JITO_TIP_ACCOUNTS: [&str; 8] = [
    "3AVi9Tg9Uo68tJfuvoKvqKNWKkC5wPdSSdeBnizKZ6jT",
    "HFqU5x63VTqvQss8hp11i4wVV8bD44PvwucfZ2bU7gRe",
    "Cw8CFyM9FkoMi7K7Crf6HNQqf4uEMzpKw6QNghXLvLkY",
    "DfXygSm4jCyNCybVYYK6DwvWqjKee8pbDmJGcLWNDXjh",
    "96gYZGLnJYVFmbjzopPSU6QiEV5fGqZNyN9nmNhvrZU5",
    "DttWaMuVvTiduZRnguLF7jNxTgiMBZ1hyAumKUiL2KRL",
    "ADuUkR4vqLUMWXxW9gh6D6L8pMSawimctcNZ5pGwDcEt",
    "ADaUMid9yfUytqMBgopwjb2DTLSokTSzL1zt6iGPaS49",
];

/// Select a random Jito tip account to reduce contention.
///
/// Jito recommends distributing tips across their 8 tip accounts to avoid
/// contention on any single account during high-traffic periods.
pub fn random_jito_tip_account() -> &'static str {
    use rand::Rng;
    let idx = rand::thread_rng().gen_range(0..JITO_TIP_ACCOUNTS.len());
    JITO_TIP_ACCOUNTS[idx]
}

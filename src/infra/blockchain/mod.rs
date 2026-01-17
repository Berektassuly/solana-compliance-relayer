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

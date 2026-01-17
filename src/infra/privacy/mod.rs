//! Privacy analytics and protection features.
//!
//! This module provides privacy-enhancing services for confidential transfers:
//! - Anonymity set health checks (assess network activity before submission)
//! - Smart delay mechanisms for timing attack mitigation
//!
//! # Ghost Mode Integration
//! Works alongside the QuickNode Ghost Mode (Jito bundles) to provide
//! comprehensive privacy protection for confidential transfers.

pub mod health_check;

pub use health_check::{AnonymitySetHealth, PrivacyHealthCheckConfig, PrivacyHealthCheckService};

//! The API layer, containing web handlers and routing.

pub mod admin;
pub mod audit;
pub mod checkout;
pub mod handlers;
pub mod router;

pub use admin::{
    AddBlocklistRequest, BlocklistEntryResponse, BlocklistResponse, ListBlocklistResponse,
    add_blocklist_handler, list_blocklist_handler, remove_blocklist_handler,
};
pub use audit::get_transfer_audit_report_handler;
pub use checkout::{
    create_checkout_session_handler, get_checkout_session_handler, submit_checkout_transfer_handler,
};
pub use handlers::ApiDoc;
pub use router::{RateLimitConfig, create_router, create_router_with_rate_limit};

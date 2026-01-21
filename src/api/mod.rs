//! The API layer, containing web handlers and routing.

pub mod admin;
pub mod handlers;
pub mod router;

pub use admin::{
    AddBlocklistRequest, BlocklistEntryResponse, BlocklistResponse, ListBlocklistResponse,
    add_blocklist_handler, list_blocklist_handler, remove_blocklist_handler,
};
pub use handlers::ApiDoc;
pub use router::{RateLimitConfig, create_router, create_router_with_rate_limit};

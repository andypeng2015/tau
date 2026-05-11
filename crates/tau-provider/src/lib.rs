//! Provider management: authentication, OAuth flows, and model listing.
//!
//! Supports multiple named provider instances with API key or OAuth
//! credentials stored in `~/.local/share/tau/auth.json`.

pub mod oauth;
pub mod resolver;
pub mod storage;

mod cli;

pub use cli::run;
pub use resolver::resolve;

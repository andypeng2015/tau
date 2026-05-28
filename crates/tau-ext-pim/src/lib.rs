//! Standard personal information management extension.
//!
//! The extension currently exposes the existing controlled `email` tool. The
//! crate is named for the broader PIM surface so calendar support can live next
//! to email without changing the extension boundary again.

pub mod email;

pub use email::run_stdio;

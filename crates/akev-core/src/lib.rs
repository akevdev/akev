//! # akev-core
//!
//! The shared vocabulary of Akev. Every other crate (proxy engine, rules engine, store,
//! API, interceptors) and the web UI (via JSON) speak in terms of the types defined here.
//!
//! Design principles:
//! * Everything that crosses the API boundary is `serde` (de)serializable with
//!   `camelCase` field names and `kebab-case` enum tags, so the TypeScript UI can mirror it 1:1.
//! * Header order and casing are preserved exactly as seen on the wire ([`http::Headers`]).
//! * Bodies are never stored inline in summaries; they're referenced and fetched lazily.

pub mod access;
pub mod body;
pub mod breakpoint;
pub mod events;
pub mod exchange;
pub mod hostpat;
pub mod http;
pub mod ids;
pub mod interceptor;
pub mod license;
pub mod queue;
pub mod redact;
pub mod rules;
pub mod settings;
pub mod time;
pub mod transparent;

pub use body::*;
pub use breakpoint::*;
pub use events::*;
pub use exchange::*;
pub use http::*;
pub use ids::*;
pub use time::*;

/// Product name, used in generated certificates, headers and UI.
pub const PRODUCT_NAME: &str = "Akev";
/// Crate/app version.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
/// Release date of this build (`YYYY-MM-DD`). A Pro licence covers builds released up to
/// its `updatesUntil` date. Bump it with [`VERSION`] (or set `AKEV_RELEASE_DATE` when
/// building a release).
pub const RELEASE_DATE: &str = match option_env!("AKEV_RELEASE_DATE") {
    Some(date) => date,
    None => "2026-11-19",
};

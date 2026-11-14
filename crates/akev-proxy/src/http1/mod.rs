//! Custom HTTP/1.x implementation: byte-faithful parsing (header casing, order and duplicates
//! preserved), body framing, keep-alive & pipelining, `Expect: 100-continue`, CONNECT and
//! upgrades.

pub mod decode;
pub mod parse;
pub(crate) mod server;
pub mod write;

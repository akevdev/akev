//! Hostname patterns: see [`akev_core::hostpat`] (shared by the proxy, the store and the
//! settings validation).

pub(crate) use akev_core::hostpat::{
    HostPattern, normalize_host, parse_patterns, split_host_port, wildcard_match,
};

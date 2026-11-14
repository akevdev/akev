//! Error construction and classification helpers shared by the upstream submodules.
//!
//! Every failure leaving the upstream module is an [`UpstreamError`] with a precise
//! [`ErrorKind`] (drives the UI icon/help text), a human readable message and a stable code
//! (`ECONNREFUSED`, `CERT_HAS_EXPIRED`, `TLS_ALERT_HANDSHAKE_FAILURE`, ...).

use super::UpstreamError;
use akev_core::ErrorKind;
use std::io;
use std::time::Duration;

impl UpstreamError {
    /// Creates an error from its parts.
    pub fn new(kind: ErrorKind, message: impl Into<String>, code: Option<&str>) -> Self {
        UpstreamError {
            kind,
            message: message.into(),
            code: code.map(str::to_owned),
        }
    }
}

/// Shorthand for an error with a code.
pub(crate) fn error(kind: ErrorKind, message: impl Into<String>, code: &str) -> UpstreamError {
    UpstreamError {
        kind,
        message: message.into(),
        code: Some(code.to_owned()),
    }
}

/// Shorthand for an internal error (misuse / invalid input rather than a network failure).
pub(crate) fn internal(message: impl Into<String>) -> UpstreamError {
    UpstreamError {
        kind: ErrorKind::Internal,
        message: message.into(),
        code: None,
    }
}

/// Malformed upstream response.
pub(crate) fn protocol(message: impl Into<String>) -> UpstreamError {
    error(
        ErrorKind::UpstreamProtocolError,
        message,
        "HPE_INVALID_RESPONSE",
    )
}

/// Timeout with a message naming what timed out.
pub(crate) fn timeout(what: impl std::fmt::Display, after: Duration) -> UpstreamError {
    error(
        ErrorKind::Timeout,
        format!("{what} timed out after {} ms", after.as_millis()),
        "ETIMEDOUT",
    )
}

/// Classify an IO error into an [`ErrorKind`].
pub(crate) fn io_kind(e: &io::Error) -> ErrorKind {
    use io::ErrorKind as K;
    match e.kind() {
        K::ConnectionRefused => ErrorKind::ConnectionRefused,
        K::ConnectionReset | K::ConnectionAborted | K::BrokenPipe | K::UnexpectedEof => {
            ErrorKind::ConnectionReset
        }
        K::HostUnreachable | K::NetworkUnreachable | K::NetworkDown | K::AddrNotAvailable => {
            ErrorKind::HostUnreachable
        }
        K::TimedOut => ErrorKind::Timeout,
        _ => match e.raw_os_error() {
            // EHOSTDOWN (Linux 112, macOS 64) is not mapped by std.
            Some(112) if cfg!(target_os = "linux") => ErrorKind::HostUnreachable,
            Some(64) if cfg!(target_os = "macos") => ErrorKind::HostUnreachable,
            _ => ErrorKind::Internal,
        },
    }
}

/// Stable (Node/libuv style) code for an IO error.
pub(crate) fn io_code(e: &io::Error) -> &'static str {
    use io::ErrorKind as K;
    match e.kind() {
        K::ConnectionRefused => "ECONNREFUSED",
        K::ConnectionReset => "ECONNRESET",
        K::ConnectionAborted => "ECONNABORTED",
        K::BrokenPipe => "EPIPE",
        K::UnexpectedEof => "ECONNRESET",
        K::TimedOut => "ETIMEDOUT",
        K::HostUnreachable => "EHOSTUNREACH",
        K::NetworkUnreachable => "ENETUNREACH",
        K::NetworkDown => "ENETDOWN",
        K::AddrNotAvailable => "EADDRNOTAVAIL",
        K::AddrInUse => "EADDRINUSE",
        K::PermissionDenied => "EACCES",
        K::InvalidData => "EPROTO",
        _ => "EIO",
    }
}

/// Short human description of an IO error.
pub(crate) fn io_description(e: &io::Error) -> String {
    use io::ErrorKind as K;
    let fixed = match e.kind() {
        K::ConnectionRefused => "connection refused",
        K::ConnectionReset => "connection reset by peer",
        K::ConnectionAborted => "connection aborted",
        K::BrokenPipe => "connection closed by peer (broken pipe)",
        K::UnexpectedEof => "connection closed unexpectedly",
        K::TimedOut => "connection timed out",
        K::HostUnreachable => "host unreachable",
        K::NetworkUnreachable => "network unreachable",
        K::NetworkDown => "network is down",
        K::AddrNotAvailable => "address not available",
        _ => return e.to_string(),
    };
    fixed.to_owned()
}

/// Convert an IO error into an [`UpstreamError`], prefixing the message with `context`.
pub(crate) fn from_io(e: &io::Error, context: impl std::fmt::Display) -> UpstreamError {
    // An UpstreamError smuggled through an io::Error (e.g. from a body stream) is unwrapped.
    if let Some(inner) = e.get_ref().and_then(|i| i.downcast_ref::<UpstreamError>()) {
        return inner.clone();
    }
    if let Some(tls) = e.get_ref().and_then(|i| i.downcast_ref::<rustls::Error>()) {
        return super::tls::map_tls_io_error(tls, &context.to_string());
    }
    if e.kind() == io::ErrorKind::InvalidData
        && let Some(mapped) = super::tls::alert_from_message(&e.to_string(), &context.to_string())
    {
        return mapped;
    }
    error(
        io_kind(e),
        format!("{context}: {}", io_description(e)),
        io_code(e),
    )
}

/// Like [`from_io`] but with a forced kind (e.g. [`ErrorKind::UpstreamProxyFailed`]).
pub(crate) fn from_io_as(
    kind: ErrorKind,
    e: &io::Error,
    context: impl std::fmt::Display,
) -> UpstreamError {
    error(
        kind,
        format!("{context}: {}", io_description(e)),
        io_code(e),
    )
}

/// Re-tag an error as an upstream proxy failure, prefixing the message.
pub(crate) fn as_proxy_failure(mut e: UpstreamError, proxy: &str) -> UpstreamError {
    if e.kind != ErrorKind::UpstreamProxyFailed {
        e.message = format!("upstream proxy {proxy}: {}", e.message);
        e.kind = ErrorKind::UpstreamProxyFailed;
    }
    e
}

/// Rank used to pick the most informative error out of several connection attempts
/// (higher wins).
pub(crate) fn io_rank(e: &io::Error) -> u8 {
    use io::ErrorKind as K;
    match e.kind() {
        K::ConnectionRefused => 5,
        K::TimedOut => 4,
        K::ConnectionReset | K::ConnectionAborted => 3,
        K::HostUnreachable | K::NetworkUnreachable | K::NetworkDown | K::AddrNotAvailable => 1,
        _ => 2,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn io_classification() {
        let e = io::Error::from(io::ErrorKind::ConnectionRefused);
        assert_eq!(io_kind(&e), ErrorKind::ConnectionRefused);
        assert_eq!(io_code(&e), "ECONNREFUSED");
        let u = from_io(&e, "connect to a:1");
        assert_eq!(u.message, "connect to a:1: connection refused");
        assert_eq!(u.code.as_deref(), Some("ECONNREFUSED"));

        let e = io::Error::from(io::ErrorKind::ConnectionReset);
        assert_eq!(io_kind(&e), ErrorKind::ConnectionReset);
        let e = io::Error::from(io::ErrorKind::NetworkUnreachable);
        assert_eq!(io_kind(&e), ErrorKind::HostUnreachable);
        assert_eq!(io_code(&e), "ENETUNREACH");
        let e = io::Error::from(io::ErrorKind::TimedOut);
        assert_eq!(io_kind(&e), ErrorKind::Timeout);
    }

    #[test]
    fn wrapped_upstream_error_is_unwrapped() {
        let inner = error(ErrorKind::ClientAborted, "client went away", "ECLIENT");
        let e = io::Error::other(inner.clone());
        let back = from_io(&e, "ignored");
        assert_eq!(back.kind, ErrorKind::ClientAborted);
        assert_eq!(back.message, "client went away");
    }

    #[test]
    fn tls_errors_inside_io_errors_are_tls_failures() {
        let e = io::Error::new(
            io::ErrorKind::InvalidData,
            rustls::Error::AlertReceived(rustls::AlertDescription::CertificateRequired),
        );
        let u = from_io(&e, "reading response from h:443");
        assert_eq!(u.kind, ErrorKind::UpstreamTlsFailed);
        assert_eq!(u.code.as_deref(), Some("TLS_ALERT_CERTIFICATE_REQUIRED"));
        assert!(u.message.contains("requires a client certificate"));

        // Same error with its source stripped (as h2 reports it).
        let e = io::Error::new(
            io::ErrorKind::InvalidData,
            "received fatal alert: BadCertificate",
        );
        let u = from_io(&e, "HTTP/2 connection to h:443");
        assert_eq!(u.kind, ErrorKind::UpstreamTlsFailed);
        assert_eq!(u.code.as_deref(), Some("TLS_ALERT_BAD_CERTIFICATE"));
        assert!(u.message.contains("rejected the client certificate"));
    }

    #[test]
    fn ranks_prefer_refused_over_unreachable() {
        let refused = io::Error::from(io::ErrorKind::ConnectionRefused);
        let unreachable = io::Error::from(io::ErrorKind::NetworkUnreachable);
        assert!(io_rank(&refused) > io_rank(&unreachable));
    }
}

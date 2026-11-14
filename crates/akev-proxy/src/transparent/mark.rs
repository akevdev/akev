//! Loop protection by firewall mark: with `settings.proxy.transparent.mark` set, every
//! outgoing upstream socket gets `SO_MARK`, so redirect rules can skip Akev's own traffic
//! (`meta mark MARK return`). Setting `SO_MARK` needs `CAP_NET_ADMIN`; without it the sockets
//! stay unmarked and the status explains why (redirect rules should then exclude Akev by uid
//! or cgroup instead).

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

/// Mark applied to upstream sockets (0 = none). Process-wide: upstream sockets are created deep
/// inside the connection pool, and one process runs one proxy.
static MARK: AtomicU32 = AtomicU32::new(0);
/// Whether a failed `SO_MARK` was already logged (once per configured mark).
static WARNED: AtomicBool = AtomicBool::new(false);

/// The mark currently applied to new upstream sockets.
#[cfg(any(target_os = "linux", test))]
pub(crate) fn current() -> u32 {
    MARK.load(Ordering::Relaxed)
}

/// Use `mark` for new upstream sockets (0 = stop marking). Returns why marking can't work, if
/// it can't (checked on a throwaway socket).
pub(crate) fn configure(mark: u32) -> Option<String> {
    if MARK.swap(mark, Ordering::Relaxed) != mark {
        WARNED.store(false, Ordering::Relaxed);
    }
    if mark == 0 {
        return None;
    }
    probe(mark).err()
}

/// Stop marking if `mark` is still the configured one (proxy shutdown).
pub(crate) fn release(mark: u32) {
    if mark != 0 {
        let _ = MARK.compare_exchange(mark, 0, Ordering::Relaxed, Ordering::Relaxed);
    }
}

#[cfg(target_os = "linux")]
fn probe(mark: u32) -> Result<(), String> {
    let socket = socket2::Socket::new(socket2::Domain::IPV4, socket2::Type::STREAM, None)
        .map_err(|e| format!("can't create a socket to test SO_MARK: {e}"))?;
    socket.set_mark(mark).map_err(|e| describe(&e))
}

#[cfg(not(target_os = "linux"))]
fn probe(_mark: u32) -> Result<(), String> {
    Err("firewall marks (SO_MARK) only exist on Linux; upstream sockets are not marked".into())
}

/// Human explanation of a failed `setsockopt(SO_MARK)`.
#[cfg(any(target_os = "linux", test))]
pub(crate) fn describe(e: &std::io::Error) -> String {
    if e.kind() == std::io::ErrorKind::PermissionDenied {
        "Akev lacks CAP_NET_ADMIN, so its upstream sockets can't be marked (SO_MARK). \
         Grant it (sudo setcap cap_net_admin+ep <akev binary>) or exclude Akev from the \
         redirect rules by user id instead."
            .to_string()
    } else {
        format!("setting SO_MARK failed: {e}")
    }
}

/// Mark an outgoing socket before it connects (no-op without a configured mark). Failures are
/// logged once and otherwise ignored: the connection still works, it just isn't excluded from
/// mark-based redirect rules.
#[cfg(target_os = "linux")]
pub(crate) fn apply<S: std::os::fd::AsFd>(socket: &S) {
    let mark = current();
    if mark == 0 {
        return;
    }
    if let Err(e) = socket2::SockRef::from(socket).set_mark(mark)
        && !WARNED.swap(true, Ordering::Relaxed)
    {
        tracing::warn!(
            "can't mark upstream sockets with {mark:#x}: {}",
            describe(&e)
        );
    }
}

/// Mark an outgoing socket (marks are Linux-only: no-op).
#[cfg(not(target_os = "linux"))]
pub(crate) fn apply<S>(_socket: &S) {}

#[cfg(test)]
mod tests {
    use super::*;

    /// One test touches the process-wide mark, so other tests never see it change mid-way.
    #[test]
    fn configure_probe_apply_release() {
        assert_eq!(configure(0), None, "no mark, nothing to check");
        assert_eq!(current(), 0);
        let err = configure(0x1a4b);
        assert_eq!(current(), 0x1a4b);
        #[cfg(target_os = "linux")]
        {
            // Unprivileged test runs get EPERM; a run with CAP_NET_ADMIN marks fine.
            if let Some(e) = &err {
                assert!(e.contains("CAP_NET_ADMIN") || e.contains("SO_MARK"), "{e}");
            }
            let socket = tokio::net::TcpSocket::new_v4();
            if let Ok(socket) = socket {
                apply(&socket); // never panics, logs at most once
                apply(&socket);
                if err.is_none() {
                    assert_eq!(socket2::SockRef::from(&socket).mark().unwrap(), 0x1a4b);
                }
            }
        }
        #[cfg(not(target_os = "linux"))]
        assert!(err.is_some());
        release(0x9999); // someone else's mark: untouched
        assert_eq!(current(), 0x1a4b);
        release(0x1a4b);
        assert_eq!(current(), 0);
        let denied = std::io::Error::from(std::io::ErrorKind::PermissionDenied);
        assert!(describe(&denied).contains("setcap cap_net_admin+ep"));
        assert!(describe(&std::io::Error::other("x")).contains("SO_MARK failed"));
    }
}

//! Internet-connectivity tracker for the shell's offline hint.
//!
//! Port of `Services/NetworkMonitor.cs`. The Windows client subscribed to OS
//! connectivity events; on Linux we keep it dependency-light and poll instead —
//! a short TCP connect to a couple of well-known anycast resolvers on :443.
//! Using raw IPs avoids a DNS dependency (DNS can resolve from cache while the
//! link is down, giving false positives), and :443 is chosen because these
//! resolvers answer DNS-over-HTTPS there, so a successful TCP handshake is a
//! reliable "the internet is reachable" signal.
//!
//! The probe is `async` and returns a `Send` future, so the shell runs it on the
//! tokio runtime (off the GTK main loop) and applies the result via [`set_online`]
//! back on the UI thread.
//!
//! [`set_online`]: NetworkMonitor::set_online

use std::sync::atomic::{AtomicBool, Ordering};

/// Well-known endpoints probed for reachability (Cloudflare and Google public
/// resolvers, both listening on 443 for DoH).
const PROBE_TARGETS: &[&str] = &["1.1.1.1:443", "8.8.8.8:443"];

/// Per-attempt connect timeout.
const PROBE_TIMEOUT_SECS: u64 = 3;

/// Tracks the last-known online/offline state.
///
/// Starts optimistic (`online == true`) — exactly like the Windows fallback,
/// which assumed connectivity and let real requests surface failures — so the
/// offline hint never flashes on a cold start before the first probe completes.
pub struct NetworkMonitor {
    online: AtomicBool,
}

impl NetworkMonitor {
    pub fn new() -> Self {
        Self {
            online: AtomicBool::new(true),
        }
    }

    /// The last-known connectivity state.
    #[allow(dead_code)]
    pub fn is_online(&self) -> bool {
        self.online.load(Ordering::Relaxed)
    }

    /// Store a freshly-observed connectivity state.
    ///
    /// Returns `true` if the state *changed* (so the caller can update the
    /// offline hint only on transitions, like the Windows `StatusChanged` event).
    pub fn set_online(&self, now: bool) -> bool {
        self.online.swap(now, Ordering::Relaxed) != now
    }

    /// Probe connectivity by attempting a short TCP connect to any probe target.
    ///
    /// Returns `true` as soon as one target accepts the connection; `false` only
    /// if every target fails or times out (a strong offline signal). This is an
    /// associated fn (no `&self`) so it can be shipped to the tokio runtime.
    pub async fn probe() -> bool {
        for target in PROBE_TARGETS {
            if Self::try_connect(target).await {
                return true;
            }
        }
        false
    }

    async fn try_connect(addr: &str) -> bool {
        use tokio::time::{timeout, Duration};
        matches!(
            timeout(
                Duration::from_secs(PROBE_TIMEOUT_SECS),
                tokio::net::TcpStream::connect(addr),
            )
            .await,
            Ok(Ok(_))
        )
    }
}

impl Default for NetworkMonitor {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn starts_online() {
        assert!(NetworkMonitor::new().is_online());
    }

    #[test]
    fn set_online_reports_transitions_only() {
        let m = NetworkMonitor::new();
        // Same as current state → no change.
        assert!(!m.set_online(true));
        // Going offline is a change.
        assert!(m.set_online(false));
        assert!(!m.is_online());
        // Staying offline → no change.
        assert!(!m.set_online(false));
        // Back online is a change.
        assert!(m.set_online(true));
        assert!(m.is_online());
    }

    #[tokio::test]
    async fn connect_to_unreachable_port_is_false() {
        // Reserved TEST-NET-1 address (RFC 5737); never routable, so the connect
        // must fail/time out and report offline — exercising the failure path
        // without depending on real internet in CI.
        assert!(!NetworkMonitor::try_connect("192.0.2.1:9").await);
    }
}

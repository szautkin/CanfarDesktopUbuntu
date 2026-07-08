//! Global "session expired" signal.
//!
//! When any request layer maps an HTTP 401/403 to [`ApiError::Unauthorized`], it
//! emits on this channel so the shell can recover the session exactly once —
//! attempting a silent re-auth and, failing that, prompting the user to sign in.
//! Mirrors the Windows `OnUnauthorized`/`OnTokenExpired` behaviour (commit eecbad7)
//! so a mid-session token expiry always leads back to sign-in instead of being
//! swallowed as a generic network error.
//!
//! [`ApiError::Unauthorized`]: crate::services::api_error::ApiError::Unauthorized

use once_cell::sync::Lazy;
use tokio::sync::broadcast;

static SENDER: Lazy<broadcast::Sender<()>> = Lazy::new(|| broadcast::channel(8).0);

/// Subscribe to session-expiry notifications (the shell does this once at startup).
pub fn subscribe() -> broadcast::Receiver<()> {
    SENDER.subscribe()
}

/// Signal that a request observed an authentication failure. Safe to call from
/// any thread; a no-op if there are no subscribers.
pub fn notify_unauthorized() {
    let _ = SENDER.send(());
}

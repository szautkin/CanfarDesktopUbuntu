use chrono::{DateTime, Utc};
use std::collections::HashMap;
use std::fmt;
use std::sync::{Arc, RwLock};

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ServiceName {
    Auth,
    Sessions,
    VoSpace,
    Tap,
    Resolver,
}

impl fmt::Display for ServiceName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ServiceName::Auth => write!(f, "Auth (CADC)"),
            ServiceName::Sessions => write!(f, "Sessions (skaha)"),
            ServiceName::VoSpace => write!(f, "Storage (VOSpace)"),
            ServiceName::Tap => write!(f, "Archive (TAP)"),
            ServiceName::Resolver => write!(f, "Name Resolver"),
        }
    }
}

impl ServiceName {
    pub fn icon_name(&self) -> &'static str {
        match self {
            ServiceName::Auth => "dialog-password-symbolic",
            ServiceName::Sessions => "computer-symbolic",
            ServiceName::VoSpace => "drive-multidisk-symbolic",
            ServiceName::Tap => "system-search-symbolic",
            ServiceName::Resolver => "find-location-symbolic",
        }
    }

    pub fn all() -> &'static [ServiceName] {
        &[
            ServiceName::Auth,
            ServiceName::Sessions,
            ServiceName::VoSpace,
            ServiceName::Tap,
            ServiceName::Resolver,
        ]
    }
}

#[derive(Debug, Clone)]
pub enum ServiceStatus {
    Unknown,
    Reachable,
    Unreachable {
        since: DateTime<Utc>,
        reason: String,
    },
}

impl ServiceStatus {
    #[cfg(test)]
    pub fn is_reachable(&self) -> bool {
        matches!(self, ServiceStatus::Reachable)
    }

    pub fn is_unreachable(&self) -> bool {
        matches!(self, ServiceStatus::Unreachable { .. })
    }
}

/// Thread-safe tracker for service connectivity.
/// Writable from tokio tasks, readable from the GLib UI thread.
#[derive(Clone)]
pub struct ServiceHealthTracker {
    inner: Arc<RwLock<HashMap<ServiceName, ServiceStatus>>>,
}

impl ServiceHealthTracker {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    pub fn set(&self, service: ServiceName, status: ServiceStatus) {
        if let Ok(mut map) = self.inner.write() {
            map.insert(service, status);
        }
    }

    pub fn get(&self, service: &ServiceName) -> ServiceStatus {
        self.inner
            .read()
            .ok()
            .and_then(|m| m.get(service).cloned())
            .unwrap_or(ServiceStatus::Unknown)
    }

    #[cfg(test)]
    pub fn any_unreachable(&self) -> bool {
        self.inner
            .read()
            .map(|m| m.values().any(|s| s.is_unreachable()))
            .unwrap_or(false)
    }

    pub fn unreachable_count(&self) -> usize {
        self.inner
            .read()
            .map(|m| m.values().filter(|s| s.is_unreachable()).count())
            .unwrap_or(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_unknown() {
        let tracker = ServiceHealthTracker::new();
        assert!(matches!(
            tracker.get(&ServiceName::Tap),
            ServiceStatus::Unknown
        ));
    }

    #[test]
    fn set_and_get() {
        let tracker = ServiceHealthTracker::new();
        tracker.set(ServiceName::Tap, ServiceStatus::Reachable);
        assert!(tracker.get(&ServiceName::Tap).is_reachable());
    }

    #[test]
    fn any_unreachable_tracks() {
        let tracker = ServiceHealthTracker::new();
        assert!(!tracker.any_unreachable());

        tracker.set(
            ServiceName::VoSpace,
            ServiceStatus::Unreachable {
                since: Utc::now(),
                reason: "timeout".into(),
            },
        );
        assert!(tracker.any_unreachable());
        assert_eq!(tracker.unreachable_count(), 1);
    }
}

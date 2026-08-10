//! PII-safe audit sink for MCP tool dispatch. Port of `Mcp/Audit/Audit.cs`.
//!
//! One [`AuditRecord`] is emitted per tool dispatch. The raw arguments are
//! **NEVER** stored — only a SHA-256 payload hash — so credentials, queries and
//! other sensitive fields can't leak into the audit trail. Records live in a
//! bounded ring ([`RingAuditSink`]) so an in-app activity view (or a test) can
//! read the most recent N without the buffer growing without bound.

use sha2::{Digest, Sha256};
use std::collections::VecDeque;
use std::sync::Mutex;

/// One PII-safe audit record per tool dispatch. Contains only a SHA-256 hex
/// digest of the payload (`payload_sha256`) — never the raw arguments.
#[derive(Clone, Debug, serde::Serialize)]
pub struct AuditRecord {
    pub request_id: String,
    pub origin: String,
    pub tool: String,
    pub verb: String,
    pub outcome: String,
    pub duration_ms: u64,
    pub payload_sha256: String,
}

/// A destination for audit records. Implementations must be cheap and
/// non-blocking on the dispatch path.
pub trait AuditSink: Send + Sync {
    fn record(&self, rec: AuditRecord);
}

/// In-memory, bounded audit sink. Keeps at most `cap` most-recent records;
/// pushing beyond `cap` evicts the oldest. Used for tests and an in-app
/// activity ring.
pub struct RingAuditSink {
    buf: Mutex<VecDeque<AuditRecord>>,
    cap: usize,
}

impl RingAuditSink {
    /// Create a ring holding at most `cap` records. A `cap` of 0 is clamped to
    /// 1 so the buffer always retains the latest record.
    pub fn new(cap: usize) -> Self {
        Self {
            buf: Mutex::new(VecDeque::with_capacity(cap.min(1024))),
            cap: cap.max(1),
        }
    }

    /// Return up to the newest `n` records, oldest-first within that window.
    pub fn recent(&self, n: usize) -> Vec<AuditRecord> {
        let buf = self.buf.lock().unwrap();
        let start = buf.len().saturating_sub(n);
        buf.iter().skip(start).cloned().collect()
    }

    /// Number of records currently retained.
    pub fn len(&self) -> usize {
        self.buf.lock().unwrap().len()
    }

    /// Whether the ring currently holds no records.
    pub fn is_empty(&self) -> bool {
        self.buf.lock().unwrap().is_empty()
    }
}

impl AuditSink for RingAuditSink {
    fn record(&self, rec: AuditRecord) {
        let mut buf = self.buf.lock().unwrap();
        if buf.len() >= self.cap {
            buf.pop_front();
        }
        buf.push_back(rec);
    }
}

impl Default for RingAuditSink {
    fn default() -> Self {
        Self::new(512)
    }
}

/// SHA-256 (hex, lowercase) over the canonical JSON encoding of `v`. `null` and
/// empty values hash the four-byte JSON token `null`. This is the only place a
/// payload is ever touched — the raw bytes are hashed and immediately dropped,
/// never stored.
pub fn payload_hash(v: &serde_json::Value) -> String {
    let bytes = serde_json::to_vec(v).unwrap_or_else(|_| b"null".to_vec());
    hex::encode(Sha256::digest(&bytes))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn rec(id: &str) -> AuditRecord {
        AuditRecord {
            request_id: id.to_string(),
            origin: "test".into(),
            tool: "search".into(),
            verb: "read".into(),
            outcome: "success".into(),
            duration_ms: 3,
            payload_sha256: payload_hash(&json!({"id": id})),
        }
    }

    #[test]
    fn payload_hash_is_deterministic() {
        let v = json!({"query": "select *", "n": 42});
        assert_eq!(payload_hash(&v), payload_hash(&v));
        // 32 bytes -> 64 hex chars.
        assert_eq!(payload_hash(&v).len(), 64);
    }

    #[test]
    fn payload_hash_differs_by_content() {
        assert_ne!(
            payload_hash(&json!({"a": 1})),
            payload_hash(&json!({"a": 2}))
        );
    }

    #[test]
    fn payload_hash_null_and_empty_match_null_token() {
        let expected = hex::encode(Sha256::digest(b"null"));
        assert_eq!(payload_hash(&serde_json::Value::Null), expected);
        assert_eq!(payload_hash(&json!(null)), expected);
    }

    #[test]
    fn ring_evicts_past_cap() {
        let sink = RingAuditSink::new(3);
        for i in 0..5 {
            sink.record(rec(&i.to_string()));
        }
        assert_eq!(sink.len(), 3);
        // Oldest two ("0","1") evicted; "2","3","4" remain oldest-first.
        let ids: Vec<_> = sink.recent(10).into_iter().map(|r| r.request_id).collect();
        assert_eq!(ids, vec!["2", "3", "4"]);
    }

    #[test]
    fn recent_returns_newest_n() {
        let sink = RingAuditSink::new(10);
        for i in 0..5 {
            sink.record(rec(&i.to_string()));
        }
        let ids: Vec<_> = sink.recent(2).into_iter().map(|r| r.request_id).collect();
        assert_eq!(ids, vec!["3", "4"]);
    }

    #[test]
    fn cap_zero_is_clamped_and_default_is_512() {
        let sink = RingAuditSink::new(0);
        sink.record(rec("x"));
        sink.record(rec("y"));
        assert_eq!(sink.len(), 1);
        assert_eq!(sink.recent(1)[0].request_id, "y");

        let d = RingAuditSink::default();
        assert!(d.is_empty());
        for i in 0..600 {
            d.record(rec(&i.to_string()));
        }
        assert_eq!(d.len(), 512);
    }

    #[test]
    fn record_stores_hash_not_raw_payload() {
        let sink = RingAuditSink::new(4);
        let secret = json!({"password": "hunter2"});
        sink.record(AuditRecord {
            request_id: "1".into(),
            origin: "cli".into(),
            tool: "login".into(),
            verb: "write".into(),
            outcome: "success".into(),
            duration_ms: 1,
            payload_sha256: payload_hash(&secret),
        });
        let got = &sink.recent(1)[0];
        assert!(!got.payload_sha256.contains("hunter2"));
        assert_eq!(got.payload_sha256, payload_hash(&secret));
    }
}

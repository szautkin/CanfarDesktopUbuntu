//! Fetches CAOM-2 observation documents from
//! `caom2ops/meta?ID=caom:{collection}/{observationID}`.
//!
//! Port of `Services/CAOM2Service.cs`. Maps 401/403 → [`Caom2Status::AuthRequired`]
//! and 404 → [`Caom2Status::NotFound`] so the detail viewer can surface a polite
//! "sign in to view" rather than a generic error. Results are cached in a bounded
//! LRU keyed by observation URI.

use crate::config::ApiEndpoints;
use crate::helpers::{caom2_parser, caom2_uri};
use crate::models::caom2::CAOM2Observation;
use reqwest::Client;
use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::Duration;

const CACHE_CAPACITY: usize = 100;

/// Outcome classification for a CAOM-2 metadata fetch.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Caom2Status {
    Success,
    AuthRequired,
    NotFound,
    InvalidId,
    Parse,
    ServerError,
}

/// Result of [`CAOM2Service::get_by_publisher_id`].
pub struct Caom2Result {
    pub status: Caom2Status,
    pub observation: Option<CAOM2Observation>,
    pub error: Option<String>,
}

impl Caom2Result {
    fn ok(observation: CAOM2Observation) -> Self {
        Self {
            status: Caom2Status::Success,
            observation: Some(observation),
            error: None,
        }
    }

    fn err(status: Caom2Status, message: impl Into<String>) -> Self {
        Self {
            status,
            observation: None,
            error: Some(message.into()),
        }
    }
}

/// Bounded LRU: a `HashMap` for lookup plus a recency queue capped at `cap`.
struct LruCache {
    map: HashMap<String, CAOM2Observation>,
    order: VecDeque<String>,
    cap: usize,
}

impl LruCache {
    fn new(cap: usize) -> Self {
        Self {
            map: HashMap::new(),
            order: VecDeque::new(),
            cap,
        }
    }

    fn get(&mut self, key: &str) -> Option<CAOM2Observation> {
        let value = self.map.get(key).cloned()?;
        self.touch(key);
        Some(value)
    }

    fn touch(&mut self, key: &str) {
        if let Some(pos) = self.order.iter().position(|k| k == key) {
            self.order.remove(pos);
        }
        self.order.push_back(key.to_string());
    }

    fn insert(&mut self, key: String, value: CAOM2Observation) {
        self.map.insert(key.clone(), value);
        self.touch(&key);
        while self.order.len() > self.cap {
            if let Some(oldest) = self.order.pop_front() {
                self.map.remove(&oldest);
            }
        }
    }
}

pub struct CAOM2Service {
    client: Client,
    endpoints: Arc<ApiEndpoints>,
    cache: Mutex<LruCache>,
}

impl CAOM2Service {
    pub fn new(client: Client, endpoints: Arc<ApiEndpoints>) -> Self {
        CAOM2Service {
            client,
            endpoints,
            cache: Mutex::new(LruCache::new(CACHE_CAPACITY)),
        }
    }

    /// Resolve a publisher ID to its CAOM-2 observation document, fetching from
    /// `caom2ops/meta` (60s timeout) and caching on success.
    pub async fn get_by_publisher_id(
        &self,
        token: Option<&str>,
        publisher_id: &str,
    ) -> Caom2Result {
        let observation_uri = match caom2_uri::to_observation_uri(publisher_id) {
            Some(u) => u,
            None => {
                return Caom2Result::err(
                    Caom2Status::InvalidId,
                    format!("Cannot derive an observation URI from: {publisher_id}"),
                )
            }
        };

        // Cache hit — clone out and drop the lock before returning.
        if let Ok(mut cache) = self.cache.lock() {
            if let Some(cached) = cache.get(&observation_uri) {
                return Caom2Result::ok(cached);
            }
        }

        let url = self.endpoints.caom2_meta_url(&observation_uri);
        let mut req = self
            .client
            .get(&url)
            .header(reqwest::header::ACCEPT, "application/xml, text/xml")
            .timeout(Duration::from_secs(60));
        if let Some(t) = token {
            req = req.bearer_auth(t);
        }

        let resp = match req.send().await {
            Ok(r) => r,
            // Transport / timeout errors: the API has no dedicated variant, so
            // surface them as a server error with the underlying message.
            Err(e) => return Caom2Result::err(Caom2Status::ServerError, e.to_string()),
        };

        match resp.status().as_u16() {
            200 => {
                let xml = match resp.text().await {
                    Ok(x) => x,
                    Err(e) => return Caom2Result::err(Caom2Status::ServerError, e.to_string()),
                };
                match caom2_parser::parse(&xml) {
                    Ok(observation) => {
                        if let Ok(mut cache) = self.cache.lock() {
                            cache.insert(observation_uri, observation.clone());
                        }
                        Caom2Result::ok(observation)
                    }
                    Err(e) => Caom2Result::err(Caom2Status::Parse, e),
                }
            }
            401 | 403 => Caom2Result::err(
                Caom2Status::AuthRequired,
                "This observation requires CADC sign-in.",
            ),
            404 => Caom2Result::err(Caom2Status::NotFound, "Observation not found."),
            other => Caom2Result::err(
                Caom2Status::ServerError,
                format!("Metadata server returned HTTP {other}."),
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::AppConfig;

    fn service() -> CAOM2Service {
        let endpoints = Arc::new(ApiEndpoints::new(AppConfig::default()));
        CAOM2Service::new(Client::new(), endpoints)
    }

    /// The cache only earns its keep if ONE service instance is shared. It was
    /// constructed per call, so every lookup missed and each observation-detail
    /// open re-issued a 30–50s `caom2ops/meta` request. The service now lives in
    /// `AppServices`; these pin the LRU's own behaviour.
    #[test]
    fn lru_returns_an_inserted_entry() {
        let mut cache = LruCache::new(4);
        let obs = CAOM2Observation {
            observation_id: "obs-1".to_string(),
            ..Default::default()
        };
        cache.insert("ivo://x?1".to_string(), obs);
        assert_eq!(
            cache.get("ivo://x?1").map(|o| o.observation_id),
            Some("obs-1".to_string())
        );
        assert!(cache.get("ivo://x?2").is_none());
    }

    #[test]
    fn lru_evicts_the_least_recently_used_entry() {
        let mut cache = LruCache::new(2);
        for id in ["a", "b"] {
            cache.insert(id.to_string(), CAOM2Observation::default());
        }
        // Touch `a`, so `b` becomes the eviction candidate.
        assert!(cache.get("a").is_some());
        cache.insert("c".to_string(), CAOM2Observation::default());

        assert!(cache.get("a").is_some(), "recently used entry survived");
        assert!(cache.get("c").is_some(), "newest entry is present");
        assert!(cache.get("b").is_none(), "least recently used was evicted");
    }

    #[test]
    fn lru_reinserting_a_key_does_not_grow_the_order_queue() {
        // `touch` removes the previous position before pushing; without that the
        // queue grows unboundedly on repeat lookups of the same observation and
        // evicts live entries.
        let mut cache = LruCache::new(2);
        for _ in 0..10 {
            cache.insert("a".to_string(), CAOM2Observation::default());
        }
        cache.insert("b".to_string(), CAOM2Observation::default());
        assert!(
            cache.get("a").is_some(),
            "`a` must not have been evicted by its own re-inserts"
        );
        assert!(cache.get("b").is_some());
        assert_eq!(cache.order.len(), 2);
    }

    #[tokio::test]
    async fn invalid_publisher_id_maps_to_invalid_id() {
        let result = service().get_by_publisher_id(None, "not-a-valid-uri").await;
        assert_eq!(result.status, Caom2Status::InvalidId);
        assert!(result.observation.is_none());
    }

    #[test]
    fn lru_evicts_oldest_beyond_capacity() {
        let mut cache = LruCache::new(2);
        cache.insert("a".into(), CAOM2Observation::default());
        cache.insert("b".into(), CAOM2Observation::default());
        // Touch "a" so "b" becomes the least-recently-used.
        assert!(cache.get("a").is_some());
        cache.insert("c".into(), CAOM2Observation::default());
        assert!(cache.get("b").is_none());
        assert!(cache.get("a").is_some());
        assert!(cache.get("c").is_some());
    }
}

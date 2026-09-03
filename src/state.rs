use crate::config::ApiEndpoints;
use crate::models::{ParsedImage, UserInfo};
use crate::services::*;
use reqwest::Client;
use std::future::Future;
use std::sync::Arc;
use tokio::sync::RwLock;

pub struct AppServices {
    pub auth: AuthService,
    pub sessions: SessionService,
    pub images: ImageService,
    pub platform: PlatformService,
    pub storage: StorageService,
    pub settings: SettingsService,
    pub recent_launches: RecentLaunchService,
    pub vospace: VoSpaceService,
    pub tap: tap_service::TAPService,
    /// The TAP service's own table/column metadata, fetched on first use.
    ///
    /// Shared so the cache is shared: built per call, every `describe_tap_schema`
    /// would re-read 401 columns from CADC.
    pub tap_schema: crate::services::tap_schema_service::TapSchemaService,
    pub datalink: DataLinkService,
    /// Shared so its 100-entry LRU actually caches. Constructed per call, the
    /// cache was always empty and every observation-detail open re-issued a
    /// 30–50s `caom2ops/meta` request.
    pub caom2: crate::services::caom2_service::CAOM2Service,
    pub search_store: SearchStoreService,
    pub notifications: NotificationService,
    pub toast: ToastNotifier,
    pub health: ServiceHealthTracker,
    pub cache: CacheService,
    pub observation_store: ObservationStore,
    pub ai_guide: Arc<crate::services::ai_guide::AiGuideService>,
    pub mcp_host: Arc<crate::mcp::host::McpHost>,
    /// Persisted MCP client allow-list / seen-clients registry, shared between the
    /// approval gate and (future) settings UI.
    pub mcp_clients: Arc<crate::mcp::client_approval::McpClientApprovalStore>,
    /// Images the user added from the registry by hand.
    ///
    /// Shared: the images widget, the registry browser and the launch form all
    /// read it, and a per-call store would each hold their own idea of the
    /// list.
    pub user_images: Arc<crate::services::user_image_store::UserImageStore>,
    /// Searches the container registry behind the platform. Only ever called
    /// because the user asked — see [`registry_service`].
    ///
    /// [`registry_service`]: crate::services::registry_service
    pub registry: crate::services::registry_service::RegistryService,
    /// Per-image container-manifest discovery cache (shared with the coordinator).
    pub image_manifests: Arc<crate::services::manifest_store::JsonManifestStore>,
    /// The last finished batch jobs, kept after CANFAR has reaped them.
    pub job_history: Arc<crate::services::job_history_store::JobHistoryStore>,
    /// Applies that run too long to hold a tool call open, and their progress.
    pub jobs: Arc<crate::services::job_registry::JobRegistry>,
    /// Container-image probe orchestrator (schedules Skaha probe jobs).
    pub image_discovery:
        Arc<crate::services::image_discovery_coordinator::ImageDiscoveryCoordinator>,
    /// Live MCP auto-apply policy flag (mirrors the persisted McpSettings toggle):
    /// when false, even non-destructive agent writes queue for review instead of
    /// auto-applying. Read by the router, updated by the Settings toggle.
    pub mcp_auto_apply: Arc<std::sync::atomic::AtomicBool>,
    /// Live "follow the agent" flag: when true, an external agent tool call
    /// navigates the UI to the relevant module. Read by the router.
    pub mcp_follow_activity: Arc<std::sync::atomic::AtomicBool>,
    /// Cap on simultaneously-pending agent proposals. Immutable policy, held
    /// here rather than on the router so the router's enforcement and the
    /// `get_current_view` snapshot an agent self-throttles against are
    /// guaranteed to quote the same number.
    pub proposal_budget: crate::mcp::budget::ProposalBudget,
    pub endpoints: Arc<ApiEndpoints>,
    pub token: RwLock<Option<String>>,
    pub username: RwLock<Option<String>>,
    pub user_info: RwLock<Option<UserInfo>>,
    pub rt: tokio::runtime::Handle,
}

impl AppServices {
    pub fn new(
        rt: tokio::runtime::Handle,
    ) -> (
        Arc<Self>,
        tokio::sync::mpsc::UnboundedReceiver<notification_service::ToastMessage>,
    ) {
        let settings = SettingsService::new();
        let config = settings.load();
        let endpoints = Arc::new(ApiEndpoints::new(config));
        let client = Client::new();
        let (toast, toast_rx) = ToastNotifier::new();
        let image_manifests = Arc::new(crate::services::manifest_store::JsonManifestStore::new());
        let job_history = Arc::new(crate::services::job_history_store::JobHistoryStore::new());

        let services = Arc::new(AppServices {
            auth: AuthService::new(client.clone(), endpoints.clone()),
            sessions: SessionService::new(client.clone(), endpoints.clone()),
            images: ImageService::new(client.clone(), endpoints.clone()),
            platform: PlatformService::new(client.clone(), endpoints.clone()),
            storage: StorageService::new(client.clone(), endpoints.clone()),
            vospace: VoSpaceService::new(client.clone(), endpoints.clone()),
            tap: tap_service::TAPService::new(client.clone(), endpoints.clone()),
            tap_schema: crate::services::tap_schema_service::TapSchemaService::new(
                std::sync::Arc::new(tap_service::TAPService::new(
                    client.clone(),
                    endpoints.clone(),
                )),
            ),
            datalink: DataLinkService::new(client.clone(), endpoints.clone()),
            caom2: crate::services::caom2_service::CAOM2Service::new(
                client.clone(),
                endpoints.clone(),
            ),
            search_store: SearchStoreService::new(),
            settings,
            recent_launches: RecentLaunchService::new(),
            notifications: NotificationService::new(),
            toast,
            health: ServiceHealthTracker::new(),
            cache: CacheService::new(),
            observation_store: ObservationStore::new(),
            ai_guide: Arc::new(crate::services::ai_guide::AiGuideService::new()),
            mcp_host: Arc::new(crate::mcp::host::McpHost::new()),
            mcp_clients: Arc::new(crate::mcp::client_approval::McpClientApprovalStore::load()),
            user_images: Arc::new(crate::services::user_image_store::UserImageStore::new()),
            registry: crate::services::registry_service::RegistryService::new(client.clone()),
            image_manifests: Arc::clone(&image_manifests),
            job_history: Arc::clone(&job_history),
            jobs: Arc::new(crate::services::job_registry::JobRegistry::new()),
            image_discovery: Arc::new(
                crate::services::image_discovery_coordinator::ImageDiscoveryCoordinator::new(
                    image_manifests,
                    job_history,
                ),
            ),
            mcp_auto_apply: Arc::new(std::sync::atomic::AtomicBool::new(
                crate::services::mcp_settings_service::McpSettingsService::new()
                    .auto_apply_enabled(),
            )),
            mcp_follow_activity: Arc::new(std::sync::atomic::AtomicBool::new(
                crate::services::mcp_settings_service::McpSettingsService::new()
                    .follow_activity_enabled(),
            )),
            proposal_budget: crate::mcp::budget::ProposalBudget::default(),
            endpoints,
            token: RwLock::new(None),
            username: RwLock::new(None),
            user_info: RwLock::new(None),
            rt,
        });
        (services, toast_rx)
    }

    /// Spawn an async task on the tokio runtime and return a future that
    /// can be awaited on the GLib main loop. This bridges tokio <-> glib.
    pub fn spawn<F, T>(&self, future: F) -> impl Future<Output = T>
    where
        F: Future<Output = T> + Send + 'static,
        T: Send + 'static,
    {
        let (tx, rx) = tokio::sync::oneshot::channel();
        self.rt.spawn(async move {
            let result = future.await;
            let _ = tx.send(result);
        });
        async move { rx.await.expect("tokio task panicked") }
    }

    pub async fn set_auth(&self, token: String, username: String) {
        *self.token.write().await = Some(token);
        *self.username.write().await = Some(username);
    }

    pub async fn set_user_info(&self, info: UserInfo) {
        *self.user_info.write().await = Some(info);
    }

    pub async fn clear_auth(&self) {
        *self.token.write().await = None;
        *self.username.write().await = None;
        *self.user_info.write().await = None;
        TokenStorage::clear();
    }

    pub async fn get_token(&self) -> Option<String> {
        self.token.read().await.clone()
    }

    pub async fn get_username(&self) -> Option<String> {
        self.username.read().await.clone()
    }

    /// Every image the app offers, parsed and ready to show.
    ///
    /// The platform's own catalogue plus the ones the user added from the
    /// registry by hand. One method rather than three call sites doing the
    /// fetch-and-parse themselves, because they were already drifting: the
    /// images widget, the find-by-package dialog and the launch form each asked
    /// `/v1/image` and parsed it their own way, so an image added to the app
    /// would appear in one and not the others.
    pub async fn image_catalogue(&self, token: &str) -> Result<Vec<ParsedImage>, String> {
        let platform = self.images.get_images(token).await?;
        Ok(merge_catalogue(&platform, &self.user_images.list()))
    }

    /// Attempt silent re-authentication using credentials stored in the keyring.
    ///
    /// Returns `true` and refreshes the in-memory token if successful.
    /// Returns `false` if credentials are missing or the login request fails.
    /// This never panics; errors are handled gracefully so callers can fall back
    /// to showing the interactive login dialog.
    pub async fn try_silent_reauth(&self) -> bool {
        let (username, password) = match TokenStorage::get_credentials() {
            Some(creds) => creds,
            None => return false,
        };

        let auth_result = self.auth.login(&username, &password).await;

        if auth_result.success {
            if let Some(token) = auth_result.token {
                // Persist the refreshed token so the next cold start picks it up.
                let _ = TokenStorage::save_token(&token);
                self.set_auth(token, username).await;
                return true;
            }
        }

        false
    }
}

/// The platform's catalogue and the user's own additions as one list.
///
/// Pure, and separate from [`AppServices::image_catalogue`], so the merge rule
/// can be tested without a token, a registry, or a file on disk.
///
/// A user image whose id the platform also lists is dropped in favour of the
/// platform's own entry: Skaha's types are authoritative, and two rows for one
/// image is a thing the user would have to reason about.
pub fn merge_catalogue(
    platform: &[crate::models::RawImage],
    added: &[crate::models::RegistryImage],
) -> Vec<ParsedImage> {
    let mut raw = platform.to_vec();
    for image in added {
        if raw.iter().any(|p| p.id == image.id) {
            continue;
        }
        raw.push(image.as_raw());
    }
    crate::helpers::image_parser::ImageParser::parse_all(&raw)
}

#[cfg(test)]
mod catalogue_tests {
    use super::merge_catalogue;
    use crate::models::{RawImage, RegistryImage};

    fn platform(id: &str) -> RawImage {
        RawImage {
            id: id.into(),
            types: vec!["notebook".into()],
        }
    }

    #[test]
    fn an_added_image_joins_the_catalogue() {
        // The point of adding one: it has to show up everywhere the platform's
        // own images do — the widget, the package search, the launch form.
        let merged = merge_catalogue(
            &[platform("h/skaha/base:1")],
            &[RegistryImage::new("h/me/mine:1", &["notebook".into()])],
        );
        assert_eq!(merged.len(), 2);
        assert!(merged.iter().any(|i| i.id == "h/me/mine:1"));
    }

    #[test]
    fn the_platform_wins_a_duplicate() {
        // Reachable: someone adds an image by hand and Skaha later publishes
        // it. Two rows for one image is a thing the user has to reason about,
        // and Skaha's types are the authoritative ones.
        let merged = merge_catalogue(
            &[platform("h/skaha/base:1")],
            &[RegistryImage::new("h/skaha/base:1", &[])],
        );
        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].types, vec!["notebook"]);
    }

    #[test]
    fn no_additions_is_the_platform_catalogue_unchanged() {
        // The common case, and it must cost nothing: most users never add an
        // image, and the merge must not reorder or re-type what Skaha sent.
        let raw = [platform("a:1"), platform("b:1")];
        let merged = merge_catalogue(&raw, &[]);
        assert_eq!(merged.len(), 2);
        assert_eq!(merged[0].id, "a:1");
        assert_eq!(merged[1].id, "b:1");
    }

    #[test]
    fn an_added_image_keeps_the_types_its_labels_gave_it() {
        // This is what makes it filterable in the widget and offerable on the
        // Standard launch tab.
        let merged = merge_catalogue(
            &[],
            &[RegistryImage::new(
                "h/me/carta:1",
                &["carta".into(), "gpu".into()],
            )],
        );
        assert_eq!(merged[0].types, vec!["carta"]);
    }
}

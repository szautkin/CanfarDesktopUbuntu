use crate::config::ApiEndpoints;
use crate::models::UserInfo;
use crate::services::*;
use reqwest::Client;
use std::future::Future;
use std::sync::Arc;
use tokio::sync::RwLock;

#[allow(dead_code)]
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
    pub datalink: DataLinkService,
    /// Shared so its 100-entry LRU actually caches. Constructed per call, the
    /// cache was always empty and every observation-detail open re-issued a
    /// 30–50s `caom2ops/meta` request.
    pub caom2: crate::services::caom2_service::CAOM2Service,
    pub search_store: SearchStoreService,
    pub templates: TemplateService,
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
    /// Per-image container-manifest discovery cache (shared with the coordinator).
    pub image_manifests: Arc<crate::services::manifest_store::JsonManifestStore>,
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

        let services = Arc::new(AppServices {
            auth: AuthService::new(client.clone(), endpoints.clone()),
            sessions: SessionService::new(client.clone(), endpoints.clone()),
            images: ImageService::new(client.clone(), endpoints.clone()),
            platform: PlatformService::new(client.clone(), endpoints.clone()),
            storage: StorageService::new(client.clone(), endpoints.clone()),
            vospace: VoSpaceService::new(client.clone(), endpoints.clone()),
            tap: tap_service::TAPService::new(client.clone(), endpoints.clone()),
            datalink: DataLinkService::new(client.clone(), endpoints.clone()),
            caom2: crate::services::caom2_service::CAOM2Service::new(
                client.clone(),
                endpoints.clone(),
            ),
            search_store: SearchStoreService::new(),
            settings,
            recent_launches: RecentLaunchService::new(),
            templates: TemplateService::new(),
            notifications: NotificationService::new(),
            toast,
            health: ServiceHealthTracker::new(),
            cache: CacheService::new(),
            observation_store: ObservationStore::new(),
            ai_guide: Arc::new(crate::services::ai_guide::AiGuideService::new()),
            mcp_host: Arc::new(crate::mcp::host::McpHost::new()),
            mcp_clients: Arc::new(crate::mcp::client_approval::McpClientApprovalStore::load()),
            image_manifests: Arc::clone(&image_manifests),
            image_discovery: Arc::new(
                crate::services::image_discovery_coordinator::ImageDiscoveryCoordinator::new(
                    image_manifests,
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

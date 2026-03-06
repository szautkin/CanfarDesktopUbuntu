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
    pub endpoints: Arc<ApiEndpoints>,
    pub token: RwLock<Option<String>>,
    pub username: RwLock<Option<String>>,
    pub user_info: RwLock<Option<UserInfo>>,
    pub rt: tokio::runtime::Handle,
}

impl AppServices {
    pub fn new(rt: tokio::runtime::Handle) -> Arc<Self> {
        let settings = SettingsService::new();
        let config = settings.load();
        let endpoints = Arc::new(ApiEndpoints::new(config));
        let client = Client::new();

        Arc::new(AppServices {
            auth: AuthService::new(client.clone(), endpoints.clone()),
            sessions: SessionService::new(client.clone(), endpoints.clone()),
            images: ImageService::new(client.clone(), endpoints.clone()),
            platform: PlatformService::new(client.clone(), endpoints.clone()),
            storage: StorageService::new(client.clone(), endpoints.clone()),
            settings,
            recent_launches: RecentLaunchService::new(),
            endpoints,
            token: RwLock::new(None),
            username: RwLock::new(None),
            user_info: RwLock::new(None),
            rt,
        })
    }

    /// Spawn an async task on the tokio runtime and return a future that
    /// can be awaited on the GLib main loop. This bridges tokio ↔ glib.
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
}

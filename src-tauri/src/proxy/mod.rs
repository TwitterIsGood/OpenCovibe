pub mod handlers;
pub mod model_fetch;
pub mod router;
pub mod routing;
pub mod streaming;
pub mod translator;

use crate::models::{ProxyModelInfo, ProxyProvider, ProxyStatus};
use crate::storage;
use crate::storage::proxy_logs::ProxyLogStore;
use router::build_proxy_router;
use std::sync::Arc;
use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

/// Hot-updatable proxy configuration shared across handlers.
#[derive(Debug, Clone)]
pub struct ProxyConfig {
    pub auto_key: String,
    pub port: u16,
    pub providers: Vec<ProxyProvider>,
    /// Shared HTTP client with connection pooling for upstream requests.
    pub http_client: reqwest::Client,
    /// Log store for recording proxy requests.
    pub log_store: Arc<ProxyLogStore>,
}

/// Manages the local proxy server lifecycle.
pub struct ProxyServer {
    config: Arc<RwLock<ProxyConfig>>,
    shutdown: CancellationToken,
    handle: Option<tokio::task::JoinHandle<()>>,
}

impl ProxyServer {
    /// Start the proxy server.
    /// Binds to 127.0.0.1:{settings.proxy_port or 0}, generates an auto-key if needed.
    pub async fn start(log_store: Arc<ProxyLogStore>) -> Result<Self, String> {
        let settings = storage::settings::get_user_settings();

        // Generate auto-key if not present
        let auto_key = settings
            .proxy_auto_key
            .clone()
            .unwrap_or_else(|| Uuid::new_v4().to_string());
        if settings.proxy_auto_key.as_ref() != Some(&auto_key) {
            let _ = storage::settings::update_user_settings(serde_json::json!({
                "proxy_auto_key": &auto_key,
            }));
        }

        // Build provider list from enabled credentials
        let providers = build_providers_from_settings(&settings);

        // Bind to configured port, fallback to port 0 (OS-assigned) if occupied
        let desired_port = settings.proxy_port.unwrap_or(0);
        let listener = match tokio::net::TcpListener::bind(format!("127.0.0.1:{desired_port}")).await
        {
            Ok(l) => l,
            Err(_) if desired_port != 0 => {
                log::warn!("[proxy] port {desired_port} occupied, falling back to OS-assigned port");
                tokio::net::TcpListener::bind("127.0.0.1:0")
                    .await
                    .map_err(|e| format!("proxy bind failed: {e}"))?
            }
            Err(e) => return Err(format!("proxy bind failed: {e}")),
        };
        let actual_port = listener.local_addr().unwrap().port();

        // Persist the actual port
        if settings.proxy_port != Some(actual_port) {
            let _ = storage::settings::update_user_settings(serde_json::json!({
                "proxy_port": actual_port,
            }));
        }

        log::info!("[proxy] starting on 127.0.0.1:{actual_port}");

        // Shared HTTP client: connection pool + keep-alive for upstream providers.
        let http_client = reqwest::Client::builder()
            .connect_timeout(std::time::Duration::from_secs(5))
            .timeout(std::time::Duration::from_secs(300))
            .pool_max_idle_per_host(4)
            .pool_idle_timeout(std::time::Duration::from_secs(90))
            .build()
            .map_err(|e| format!("failed to build HTTP client: {e}"))?;

        let config = Arc::new(RwLock::new(ProxyConfig {
            auto_key: auto_key.clone(),
            port: actual_port,
            providers,
            http_client,
            log_store,
        }));

        let shutdown = CancellationToken::new();
        let cancel = shutdown.clone();
        let config_clone = config.clone();
        let app = build_proxy_router(config_clone);

        let handle = tokio::spawn(async move {
            let server = axum::serve(listener, app)
                .with_graceful_shutdown(async move { cancel.cancelled().await });
            if let Err(e) = server.await {
                log::error!("[proxy] server error: {e}");
            }
        });

        Ok(Self {
            config,
            shutdown,
            handle: Some(handle),
        })
    }

    /// Stop the proxy server gracefully.
    pub async fn stop(mut self) {
        log::info!("[proxy] stopping");
        self.shutdown.cancel();
        if let Some(h) = self.handle.take() {
            let _ = h.await;
        }
    }

    /// Check if the proxy server task is still alive.
    pub fn is_alive(&self) -> bool {
        self.handle
            .as_ref()
            .map_or(false, |h| !h.is_finished())
    }

    /// Update the provider list (hot-reload routing table).
    pub async fn update_providers(&self, providers: Vec<ProxyProvider>) {
        let mut cfg = self.config.write().await;
        log::info!("[proxy] updating providers ({} providers)", providers.len());
        cfg.providers = providers;
    }

    /// Get current proxy status for the frontend.
    /// Returns `running: false` if the server task has crashed.
    pub async fn get_status(&self) -> ProxyStatus {
        if !self.is_alive() {
            return ProxyStatus {
                running: false,
                port: 0,
                base_url: String::new(),
                auto_key: String::new(),
                models: vec![],
            };
        }
        let cfg = self.config.read().await;
        let models = aggregate_models(&cfg.providers);
        ProxyStatus {
            running: true,
            port: cfg.port,
            base_url: format!("http://127.0.0.1:{}", cfg.port),
            auto_key: cfg.auto_key.clone(),
            models,
        }
    }

    /// Refresh models from all enabled providers by fetching /v1/models.
    /// Returns the updated model list.
    pub async fn refresh_models(&self) -> Result<Vec<ProxyModelInfo>, String> {
        let cfg = self.config.read().await;
        let mut updated_providers = cfg.providers.clone();
        let client = cfg.http_client.clone();
        drop(cfg);

        for provider in &mut updated_providers {
            if !provider.enabled {
                continue;
            }
            match model_fetch::fetch_provider_models_with_client(
                &client,
                &provider.base_url,
                provider.api_key.as_deref(),
                &provider.protocol,
            )
            .await
            {
                Ok(models) => {
                    log::info!(
                        "[proxy] fetched {} models from {}",
                        models.len(),
                        provider.platform_id
                    );
                    provider.models = models;
                }
                Err(e) => {
                    log::warn!(
                        "[proxy] failed to fetch models from {}: {e}",
                        provider.platform_id
                    );
                }
            }
        }

        self.update_providers(updated_providers.clone()).await;
        Ok(aggregate_models(&updated_providers))
    }

    pub fn config(&self) -> Arc<RwLock<ProxyConfig>> {
        self.config.clone()
    }
}

/// Build ProxyProvider list from UserSettings.
fn build_providers_from_settings(settings: &crate::models::UserSettings) -> Vec<ProxyProvider> {
    let providers: Vec<ProxyProvider> = settings
        .platform_credentials
        .iter()
        .filter(|c| c.enabled.unwrap_or(true))
        .filter_map(|c| {
            let base_url = c.base_url.clone()?;
            let platform_id = c.platform_id.clone();
            let protocol = c
                .protocol
                .clone()
                .unwrap_or_else(|| "anthropic".to_string());
            let models = c.models.clone().unwrap_or_default();
            log::info!(
                "[proxy] provider: id='{}' base_url='{}' protocol='{}' models={:?}",
                platform_id,
                base_url,
                protocol,
                models,
            );
            Some(ProxyProvider {
                platform_id,
                base_url,
                api_key: c.api_key.clone(),
                protocol,
                models,
                enabled: true,
            })
        })
        .collect();
    log::info!("[proxy] built {} providers from settings", providers.len());
    providers
}

/// Aggregate models from all providers into a deduplicated list.
/// When multiple providers offer the same model, the first provider wins
/// for display, but routing still round-robins across all providers.
fn aggregate_models(providers: &[ProxyProvider]) -> Vec<ProxyModelInfo> {
    let mut seen = std::collections::HashSet::new();
    let mut result = Vec::new();
    for p in providers.iter().filter(|p| p.enabled) {
        for m in &p.models {
            if seen.insert(m.clone()) {
                result.push(ProxyModelInfo {
                    id: m.clone(),
                    platform_id: p.platform_id.clone(),
                    provider_name: p.platform_id.clone(),
                    protocol: p.protocol.clone(),
                });
            }
        }
    }
    result
}

/// Tauri managed state type for the proxy.
pub type ProxyState = Arc<tokio::sync::Mutex<Option<ProxyServer>>>;

use crate::models::{ProxyModelInfo, ProxyStatus};
use crate::proxy::{ProxyServer, ProxyState};
use crate::storage;
use tauri::State;

#[tauri::command]
pub async fn start_proxy(proxy_state: State<'_, ProxyState>) -> Result<ProxyStatus, String> {
    let mut guard = proxy_state.lock().await;

    // Stop existing proxy if running
    if let Some(existing) = guard.take() {
        existing.stop().await;
    }

    let server = ProxyServer::start().await?;
    let status = server.get_status().await;
    *guard = Some(server);

    log::info!("[proxy] started, status: port={}", status.port);
    Ok(status)
}

#[tauri::command]
pub async fn stop_proxy(proxy_state: State<'_, ProxyState>) -> Result<(), String> {
    let mut guard = proxy_state.lock().await;
    if let Some(server) = guard.take() {
        server.stop().await;
        log::info!("[proxy] stopped");
    }
    Ok(())
}

#[tauri::command]
pub async fn get_proxy_status(proxy_state: State<'_, ProxyState>) -> Result<ProxyStatus, String> {
    let guard = proxy_state.lock().await;
    match guard.as_ref() {
        Some(server) => Ok(server.get_status().await),
        None => Ok(ProxyStatus {
            running: false,
            port: 0,
            base_url: String::new(),
            auto_key: String::new(),
            models: vec![],
        }),
    }
}

#[tauri::command]
pub async fn refresh_proxy_models(
    proxy_state: State<'_, ProxyState>,
) -> Result<Vec<ProxyModelInfo>, String> {
    let guard = proxy_state.lock().await;
    match guard.as_ref() {
        Some(server) => server.refresh_models().await,
        None => Err("proxy not running".to_string()),
    }
}

#[tauri::command]
pub async fn fetch_provider_models(
    platform_id: String,
) -> Result<Vec<String>, String> {
    let settings = storage::settings::get_user_settings();

    let cred = settings
        .platform_credentials
        .iter()
        .find(|c| c.platform_id == platform_id)
        .ok_or_else(|| format!("provider not found: {platform_id}"))?;

    let base_url = cred
        .base_url
        .clone()
        .ok_or_else(|| format!("no base_url for provider: {platform_id}"))?;

    let protocol = cred
        .protocol
        .clone()
        .unwrap_or_else(|| "anthropic".to_string());

    crate::proxy::model_fetch::fetch_provider_models(
        &base_url,
        cred.api_key.as_deref(),
        &protocol,
    )
    .await
}

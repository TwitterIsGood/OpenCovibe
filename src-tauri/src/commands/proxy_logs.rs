use crate::models::{ProxyDayHealth, ProxyLogFilter, ProxyRequestLog};
use crate::storage::proxy_logs::ProxyLogStore;
use serde::Serialize;
use std::sync::Arc;
use tauri::State;

pub type ProxyLogState = Arc<ProxyLogStore>;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProxyLogResponse {
    pub entries: Vec<ProxyRequestLog>,
    pub total: u64,
}

#[tauri::command]
pub fn get_proxy_logs(
    state: State<'_, ProxyLogState>,
    filter: ProxyLogFilter,
    limit: Option<u32>,
    offset: Option<u32>,
) -> Result<ProxyLogResponse, String> {
    let limit = limit.unwrap_or(50).min(200);
    let offset = offset.unwrap_or(0);
    let (entries, total) = state.query(&filter, limit, offset);
    Ok(ProxyLogResponse { entries, total })
}

#[tauri::command]
pub fn get_proxy_health(
    state: State<'_, ProxyLogState>,
    hours: Option<u32>,
) -> Result<Vec<ProxyDayHealth>, String> {
    Ok(state.health(hours.unwrap_or(24)))
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProxyLogDistinctValues {
    pub models: Vec<String>,
    pub providers: Vec<String>,
}

#[tauri::command]
pub fn get_proxy_log_filters(
    state: State<'_, ProxyLogState>,
) -> Result<ProxyLogDistinctValues, String> {
    let (models, providers) = state.distinct_values();
    Ok(ProxyLogDistinctValues { models, providers })
}

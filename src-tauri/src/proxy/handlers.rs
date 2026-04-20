use crate::models::ProxyRequestLog;
use crate::proxy::routing;
use crate::proxy::streaming;
use crate::proxy::translator;
use crate::proxy::ProxyConfig;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use axum::response::Response;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::RwLock;

/// POST /v1/messages — main inference endpoint (Anthropic protocol inbound).
pub async fn handle_messages(
    State(config): State<Arc<RwLock<ProxyConfig>>>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    // Auth check
    let cfg = config.read().await;
    if !check_auth(&headers, &cfg.auto_key) {
        return (StatusCode::UNAUTHORIZED, "invalid proxy key").into_response();
    }

    // Parse the request body
    let req_value: serde_json::Value = match serde_json::from_slice(&body) {
        Ok(v) => v,
        Err(e) => {
            return (StatusCode::BAD_REQUEST, format!("invalid JSON: {e}")).into_response();
        }
    };

    let requested_model = req_value
        .get("model")
        .and_then(|m| m.as_str())
        .unwrap_or("");

    // Resolve routing: which provider + actual model name
    let route = match routing::resolve_route(&cfg, requested_model) {
        Some(r) => r,
        None => {
            log::error!(
                "[proxy] no provider found for model '{}'. Available providers: {:?}",
                requested_model,
                cfg.providers.iter().map(|p| format!("{}(models={:?})", p.platform_id, p.models)).collect::<Vec<_>>()
            );
            return (
                StatusCode::NOT_FOUND,
                format!("no provider found for model: {requested_model}"),
            )
                .into_response();
        }
    };

    let provider = route.provider.clone();
    let actual_model = route.actual_model.clone();
    let is_stream = req_value.get("stream").and_then(|s| s.as_bool()).unwrap_or(false);

    log::info!(
        "[proxy] routing model '{}' -> provider '{}' base_url='{}' protocol='{}' actual_model='{}' stream={}",
        requested_model,
        provider.platform_id,
        provider.base_url,
        provider.protocol,
        actual_model,
        is_stream,
    );

    // Build upstream request
    let (upstream_body, upstream_url) = if provider.protocol == "openai" {
        let translated = translator::anthropic_to_openai_request(&req_value, &actual_model);
        let url = format!(
            "{}/v1/chat/completions",
            provider.base_url.trim_end_matches('/')
        );
        (serde_json::to_vec(&translated).unwrap(), url)
    } else {
        let mut body = req_value.clone();
        if let Some(obj) = body.as_object_mut() {
            obj.insert("model".to_string(), serde_json::Value::String(actual_model.clone()));
        }
        let url = format!("{}/v1/messages", provider.base_url.trim_end_matches('/'));
        (serde_json::to_vec(&body).unwrap(), url)
    };

    // Build upstream HTTP client (shared connection pool)
    let client = cfg.http_client.clone();
    let mut req_builder = client
        .post(&upstream_url)
        .header("content-type", "application/json")
        .body(upstream_body);

    // Set auth headers based on provider config
    if let Some(ref key) = provider.api_key {
        if provider.protocol == "openai" {
            req_builder = req_builder.header("authorization", format!("Bearer {key}"));
        } else {
            req_builder = req_builder.header("x-api-key", key);
            req_builder = req_builder.header("anthropic-version", "2023-06-01");
        }
    }

    let start = Instant::now();
    let upstream_resp = match req_builder.send().await {
        Ok(r) => r,
        Err(e) => {
            let latency_ms = start.elapsed().as_millis() as u64;
            log::error!("[proxy] upstream request to {} failed: {e}", upstream_url);
            cfg.log_store.append(ProxyRequestLog {
                id: 0,
                ts: String::new(),
                model: requested_model.to_string(),
                actual_model: actual_model.clone(),
                provider_id: provider.platform_id.clone(),
                result: "error".to_string(),
                status_code: 502,
                latency_ms,
                input_tokens: None,
                output_tokens: None,
                thinking_tokens: None,
                cache_read_tokens: None,
                cache_creation_tokens: None,
                is_stream,
            });
            return (StatusCode::BAD_GATEWAY, format!("upstream error: {e}")).into_response();
        }
    };

    let latency_ms = start.elapsed().as_millis() as u64;
    let status = upstream_resp.status();
    let status_code = status.as_u16();
    let is_success = status.is_success();

    log::info!(
        "[proxy] upstream responded status={} latency={}ms",
        status_code, latency_ms
    );

    if !is_success {
        let body_text = upstream_resp.text().await.unwrap_or_default();
        cfg.log_store.append(ProxyRequestLog {
            id: 0,
            ts: String::new(),
            model: requested_model.to_string(),
            actual_model,
            provider_id: provider.platform_id.clone(),
            result: "error".to_string(),
            status_code,
            latency_ms,
            input_tokens: None,
            output_tokens: None,
            thinking_tokens: None,
            cache_read_tokens: None,
            cache_creation_tokens: None,
            is_stream,
        });
        let sc = StatusCode::from_u16(status_code).unwrap_or(StatusCode::BAD_GATEWAY);
        return (sc, body_text).into_response();
    }

    // Streaming response
    if is_stream {
        // Log without token data (tokens arrive in SSE events which we don't parse here)
        cfg.log_store.append(ProxyRequestLog {
            id: 0,
            ts: String::new(),
            model: requested_model.to_string(),
            actual_model,
            provider_id: provider.platform_id.clone(),
            result: "success".to_string(),
            status_code,
            latency_ms,
            input_tokens: None,
            output_tokens: None,
            thinking_tokens: None,
            cache_read_tokens: None,
            cache_creation_tokens: None,
            is_stream: true,
        });

        let content_type = upstream_resp
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("text/event-stream")
            .to_string();

        if provider.protocol == "openai" {
            streaming::translate_openai_to_anthropic_stream(upstream_resp).into_response()
        } else {
            streaming::passthrough_stream(upstream_resp, content_type).into_response()
        }
    } else {
        // Non-streaming response — extract token counts
        let resp_body = upstream_resp.bytes().await.unwrap_or_default();
        let resp_json: serde_json::Result<serde_json::Value> =
            serde_json::from_slice(&resp_body);

        let (input_tokens, output_tokens, thinking_tokens, cache_read, cache_creation) =
            extract_tokens(&resp_json, &provider.protocol);

        cfg.log_store.append(ProxyRequestLog {
            id: 0,
            ts: String::new(),
            model: requested_model.to_string(),
            actual_model,
            provider_id: provider.platform_id.clone(),
            result: "success".to_string(),
            status_code,
            latency_ms,
            input_tokens,
            output_tokens,
            thinking_tokens,
            cache_read_tokens: cache_read,
            cache_creation_tokens: cache_creation,
            is_stream: false,
        });

        if provider.protocol == "openai" {
            let translated = translator::openai_to_anthropic_response(&resp_body);
            (
                StatusCode::OK,
                [("content-type", "application/json")],
                translated,
            )
                .into_response()
        } else {
            (
                StatusCode::OK,
                [("content-type", "application/json")],
                resp_body.to_vec(),
            )
                .into_response()
        }
    }
}

/// Extract token counts from a response JSON (best-effort).
fn extract_tokens(
    resp: &serde_json::Result<serde_json::Value>,
    protocol: &str,
) -> (Option<u64>, Option<u64>, Option<u64>, Option<u64>, Option<u64>) {
    let Ok(val) = resp else {
        return (None, None, None, None, None);
    };

    if protocol == "openai" {
        // OpenAI format: { usage: { prompt_tokens, completion_tokens } }
        let usage = val.get("usage");
        let input = usage
            .and_then(|u| u.get("prompt_tokens"))
            .and_then(|v| v.as_u64());
        let output = usage
            .and_then(|u| u.get("completion_tokens"))
            .and_then(|v| v.as_u64());
        (input, output, None, None, None)
    } else {
        // Anthropic format: { usage: { input_tokens, output_tokens, ... } }
        let usage = val.get("usage");
        let input = usage
            .and_then(|u| u.get("input_tokens"))
            .and_then(|v| v.as_u64());
        let output = usage
            .and_then(|u| u.get("output_tokens"))
            .and_then(|v| v.as_u64());
        let cache_read = usage
            .and_then(|u| u.get("cache_read_input_tokens"))
            .and_then(|v| v.as_u64());
        let cache_creation = usage
            .and_then(|u| u.get("cache_creation_input_tokens"))
            .and_then(|v| v.as_u64());
        (input, output, None, cache_read, cache_creation)
    }
}

/// GET /v1/models — return aggregated model list.
pub async fn handle_models(
    State(config): State<Arc<RwLock<ProxyConfig>>>,
    headers: HeaderMap,
) -> Response {
    let cfg = config.read().await;
    if !check_auth(&headers, &cfg.auto_key) {
        return (StatusCode::UNAUTHORIZED, "invalid proxy key").into_response();
    }

    let mut seen = std::collections::HashSet::new();
    let models: Vec<serde_json::Value> = cfg
        .providers
        .iter()
        .filter(|p| p.enabled)
        .flat_map(|p| p.models.iter().map(move |m| m.clone()))
        .filter(|m| seen.insert(m.clone()))
        .map(|m| {
            serde_json::json!({
                "id": m,
                "object": "model",
                "created": 0,
                "owned_by": "opencovibe-proxy"
            })
        })
        .collect();

    let body = serde_json::json!({
        "object": "list",
        "data": models,
    });

    (
        StatusCode::OK,
        [("content-type", "application/json")],
        serde_json::to_string(&body).unwrap_or_default(),
    )
        .into_response()
}

/// POST /v1/messages/count_tokens — passthrough to provider.
pub async fn handle_count_tokens(
    State(config): State<Arc<RwLock<ProxyConfig>>>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    let cfg = config.read().await;
    if !check_auth(&headers, &cfg.auto_key) {
        return (StatusCode::UNAUTHORIZED, "invalid proxy key").into_response();
    }

    let provider = match cfg.providers.iter().find(|p| p.enabled && p.protocol == "anthropic") {
        Some(p) => p,
        None => return (StatusCode::NOT_FOUND, "no anthropic provider available").into_response(),
    };

    let url = format!(
        "{}/v1/messages/count_tokens",
        provider.base_url.trim_end_matches('/')
    );

    let client = cfg.http_client.clone();
    let mut req = client
        .post(&url)
        .header("content-type", "application/json")
        .body(body.to_vec());

    if let Some(ref key) = provider.api_key {
        req = req.header("x-api-key", key);
        req = req.header("anthropic-version", "2023-06-01");
    }

    match req.send().await {
        Ok(resp) => {
            let status = StatusCode::from_u16(resp.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
            let body = resp.bytes().await.unwrap_or_default();
            (status, [("content-type", "application/json")], body.to_vec()).into_response()
        }
        Err(e) => (StatusCode::BAD_GATEWAY, format!("upstream error: {e}")).into_response(),
    }
}

/// Check proxy auth header (x-api-key or Authorization: Bearer).
fn check_auth(headers: &HeaderMap, expected_key: &str) -> bool {
    if let Some(val) = headers.get("x-api-key").and_then(|v| v.to_str().ok()) {
        return val == expected_key;
    }
    if let Some(val) = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
    {
        if let Some(key) = val.strip_prefix("Bearer ") {
            return key == expected_key;
        }
    }
    false
}

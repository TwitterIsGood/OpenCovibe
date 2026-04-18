/// Fetch model lists from provider /v1/models endpoints.

/// Fetch the model list from a provider using a shared HTTP client.
pub async fn fetch_provider_models_with_client(
    client: &reqwest::Client,
    base_url: &str,
    api_key: Option<&str>,
    protocol: &str,
) -> Result<Vec<String>, String> {
    let url = format!("{}/v1/models", base_url.trim_end_matches('/'));
    let mut req = client.get(&url).timeout(std::time::Duration::from_secs(10));

    if let Some(key) = api_key {
        if protocol == "openai" {
            req = req.header("authorization", format!("Bearer {key}"));
        } else {
            req = req.header("x-api-key", key);
            req = req.header("anthropic-version", "2023-06-01");
        }
    }

    let resp = req.send().await.map_err(|e| format!("request failed: {e}"))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        if protocol == "openai" && status.as_u16() == 404 {
            return fetch_openai_models_fallback_with_client(client, base_url, api_key).await;
        }
        return Err(format!("status {status}: {body}"));
    }

    let body: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| format!("parse error: {e}"))?;

    parse_model_list(&body)
}

/// Fetch the model list from a provider (creates a one-off client).
///
/// Both Anthropic and OpenAI use the same `/v1/models` response format:
/// `{"data": [{"id": "model-name", ...}]}`
///
/// For OpenAI protocol, also tries `/models` (without /v1 prefix).
pub async fn fetch_provider_models(
    base_url: &str,
    api_key: Option<&str>,
    protocol: &str,
) -> Result<Vec<String>, String> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .map_err(|e| format!("client build failed: {e}"))?;

    let url = format!("{}/v1/models", base_url.trim_end_matches('/'));

    let mut req = client.get(&url);

    // Set auth headers
    if let Some(key) = api_key {
        if protocol == "openai" {
            req = req.header("authorization", format!("Bearer {key}"));
        } else {
            req = req.header("x-api-key", key);
            req = req.header("anthropic-version", "2023-06-01");
        }
    }

    let resp = req.send().await.map_err(|e| format!("request failed: {e}"))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        // If /v1/models fails for OpenAI, try /models
        if protocol == "openai" && status.as_u16() == 404 {
            return fetch_openai_models_fallback(base_url, api_key).await;
        }
        return Err(format!("status {status}: {body}"));
    }

    let body: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| format!("parse error: {e}"))?;

    parse_model_list(&body)
}

/// Try fetching from /models (without /v1 prefix) for OpenAI-compatible providers.
async fn fetch_openai_models_fallback_with_client(
    client: &reqwest::Client,
    base_url: &str,
    api_key: Option<&str>,
) -> Result<Vec<String>, String> {
    let url = format!("{}/models", base_url.trim_end_matches('/'));
    let mut req = client.get(&url).timeout(std::time::Duration::from_secs(10));
    if let Some(key) = api_key {
        req = req.header("authorization", format!("Bearer {key}"));
    }

    let resp = req.send().await.map_err(|e| format!("request failed: {e}"))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(format!("status {status}: {body}"));
    }

    let body: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| format!("parse error: {e}"))?;

    parse_model_list(&body)
}

/// Try fetching from /models (one-off client, for IPC commands).
async fn fetch_openai_models_fallback(
    base_url: &str,
    api_key: Option<&str>,
) -> Result<Vec<String>, String> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .map_err(|e| format!("client build failed: {e}"))?;

    let url = format!("{}/models", base_url.trim_end_matches('/'));
    let mut req = client.get(&url);
    if let Some(key) = api_key {
        req = req.header("authorization", format!("Bearer {key}"));
    }

    let resp = req.send().await.map_err(|e| format!("request failed: {e}"))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(format!("status {status}: {body}"));
    }

    let body: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| format!("parse error: {e}"))?;

    parse_model_list(&body)
}

/// Parse the standard /v1/models response format.
fn parse_model_list(body: &serde_json::Value) -> Result<Vec<String>, String> {
    let data = body
        .get("data")
        .and_then(|d| d.as_array())
        .ok_or_else(|| "missing 'data' array in response".to_string())?;

    let models: Vec<String> = data
        .iter()
        .filter_map(|item| {
            item.get("id")
                .and_then(|id| id.as_str())
                .map(|s| s.to_string())
        })
        .collect();

    Ok(models)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parse_standard_model_list() {
        let body = json!({
            "data": [
                {"id": "gpt-4", "object": "model"},
                {"id": "gpt-3.5-turbo", "object": "model"},
            ]
        });
        let models = parse_model_list(&body).unwrap();
        assert_eq!(models, vec!["gpt-4", "gpt-3.5-turbo"]);
    }

    #[test]
    fn parse_empty_list() {
        let body = json!({"data": []});
        let models = parse_model_list(&body).unwrap();
        assert!(models.is_empty());
    }

    #[test]
    fn parse_missing_data_array() {
        let body = json!({"error": "not found"});
        assert!(parse_model_list(&body).is_err());
    }

    #[test]
    fn parse_skips_items_without_id() {
        let body = json!({
            "data": [
                {"id": "model-a"},
                {"object": "model"},
                {"id": "model-b"},
            ]
        });
        let models = parse_model_list(&body).unwrap();
        assert_eq!(models, vec!["model-a", "model-b"]);
    }
}

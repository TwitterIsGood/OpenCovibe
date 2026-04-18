use crate::models::ProxyProvider;
use crate::proxy::ProxyConfig;
use std::sync::atomic::{AtomicUsize, Ordering};

/// Resolved route for a model request.
#[derive(Debug, Clone)]
pub struct ResolvedRoute {
    pub provider: ProxyProvider,
    pub actual_model: String,
}

/// Round-robin counter for load balancing when same model exists in multiple providers.
static ROUND_ROBIN: AtomicUsize = AtomicUsize::new(0);

/// Resolve a requested model name to a provider + actual model name.
///
/// Strategy:
/// 1. Direct match: find provider that has this exact model in its model list
/// 2. If multiple providers have the same model, round-robin across them
pub fn resolve_route(config: &ProxyConfig, requested_model: &str) -> Option<ResolvedRoute> {
    if requested_model.is_empty() {
        // No model specified — route to first enabled provider
        let provider = config.providers.iter().find(|p| p.enabled)?;
        let default_model = provider.models.first().cloned()?;
        return Some(ResolvedRoute {
            provider: provider.clone(),
            actual_model: default_model,
        });
    }

    // Find all providers that have this model
    let matching: Vec<&ProxyProvider> = config
        .providers
        .iter()
        .filter(|p| p.enabled && p.models.contains(&requested_model.to_string()))
        .collect();

    if matching.is_empty() {
        // No exact match — try first enabled provider (model might be valid there)
        let provider = config.providers.iter().find(|p| p.enabled)?;
        log::warn!(
            "[proxy/routing] model '{}' not found in any provider's model list, falling back to first enabled provider '{}' (base_url={})",
            requested_model,
            provider.platform_id,
            provider.base_url,
        );
        return Some(ResolvedRoute {
            provider: provider.clone(),
            actual_model: requested_model.to_string(),
        });
    }

    if matching.len() == 1 {
        let provider = matching[0];
        return Some(ResolvedRoute {
            provider: provider.clone(),
            actual_model: requested_model.to_string(),
        });
    }

    // Multiple providers: round-robin load balancing
    let idx = ROUND_ROBIN.fetch_add(1, Ordering::Relaxed) % matching.len();
    let provider = matching[idx];
    Some(ResolvedRoute {
        provider: provider.clone(),
        actual_model: requested_model.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::ProxyProvider;

    fn make_provider(id: &str, models: Vec<&str>, protocol: &str) -> ProxyProvider {
        ProxyProvider {
            platform_id: id.to_string(),
            base_url: "http://localhost".to_string(),
            api_key: None,
            protocol: protocol.to_string(),
            enabled: true,
            models: models.into_iter().map(|s| s.to_string()).collect(),
        }
    }

    fn make_config(providers: Vec<ProxyProvider>) -> ProxyConfig {
        ProxyConfig {
            auto_key: "test-key".to_string(),
            port: 12345,
            providers,
        }
    }

    #[test]
    fn route_to_single_provider() {
        let config = make_config(vec![make_provider("p1", vec!["model-a", "model-b"], "anthropic")]);
        let route = resolve_route(&config, "model-a").unwrap();
        assert_eq!(route.provider.platform_id, "p1");
        assert_eq!(route.actual_model, "model-a");
    }

    #[test]
    fn route_unknown_model_uses_first_enabled() {
        let config = make_config(vec![make_provider("p1", vec!["model-a"], "anthropic")]);
        let route = resolve_route(&config, "unknown-model").unwrap();
        assert_eq!(route.provider.platform_id, "p1");
        assert_eq!(route.actual_model, "unknown-model");
    }

    #[test]
    fn route_empty_model_uses_first_provider_default() {
        let config = make_config(vec![make_provider("p1", vec!["default-model"], "anthropic")]);
        let route = resolve_route(&config, "").unwrap();
        assert_eq!(route.actual_model, "default-model");
    }

    #[test]
    fn route_skips_disabled_provider() {
        let mut p1 = make_provider("disabled", vec!["model-a"], "anthropic");
        p1.enabled = false;
        let p2 = make_provider("enabled", vec!["model-b"], "anthropic");
        let config = make_config(vec![p1, p2]);
        let route = resolve_route(&config, "model-a").unwrap();
        assert_eq!(route.provider.platform_id, "enabled");
    }

    #[test]
    fn route_no_enabled_providers_returns_none() {
        let mut p1 = make_provider("p1", vec!["model-a"], "anthropic");
        p1.enabled = false;
        let config = make_config(vec![p1]);
        assert!(resolve_route(&config, "model-a").is_none());
    }

    #[test]
    fn route_no_providers_returns_none() {
        let config = make_config(vec![]);
        assert!(resolve_route(&config, "model-a").is_none());
    }

    #[test]
    fn route_round_robin_across_providers() {
        // Reset counter
        ROUND_ROBIN.store(0, Ordering::Relaxed);
        let config = make_config(vec![
            make_provider("p1", vec!["shared-model"], "anthropic"),
            make_provider("p2", vec!["shared-model"], "openai"),
        ]);
        let r1 = resolve_route(&config, "shared-model").unwrap();
        let r2 = resolve_route(&config, "shared-model").unwrap();
        // Should alternate
        assert_ne!(r1.provider.platform_id, r2.provider.platform_id);
    }
}

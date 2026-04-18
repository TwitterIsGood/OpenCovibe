use crate::proxy::handlers;
use crate::proxy::ProxyConfig;
use axum::Router;
use std::sync::Arc;
use tokio::sync::RwLock;

pub fn build_proxy_router(config: Arc<RwLock<ProxyConfig>>) -> Router {
    Router::new()
        .route("/v1/messages", axum::routing::post(handlers::handle_messages))
        .route("/v1/models", axum::routing::get(handlers::handle_models))
        .route(
            "/v1/messages/count_tokens",
            axum::routing::post(handlers::handle_count_tokens),
        )
        .with_state(config)
}

use axum::{Router, extract::State, http::StatusCode, routing::get};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

#[derive(Default)]
#[doc(hidden)]
pub struct RuntimeHealth {
    ready: AtomicBool,
}

impl RuntimeHealth {
    pub fn set_ready(&self, ready: bool) {
        self.ready.store(ready, Ordering::Release);
    }
}

#[doc(hidden)]
pub fn router(health: Arc<RuntimeHealth>) -> Router {
    Router::new()
        .route("/health", get(health_handler))
        .route("/ready", get(ready_handler))
        .with_state(health)
}

async fn health_handler() -> &'static str {
    "ok\n"
}

async fn ready_handler(State(health): State<Arc<RuntimeHealth>>) -> (StatusCode, &'static str) {
    if health.ready.load(Ordering::Acquire) {
        (StatusCode::OK, "ready\n")
    } else {
        (StatusCode::SERVICE_UNAVAILABLE, "not ready\n")
    }
}

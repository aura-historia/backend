mod submit;
mod types;

use crate::state::ContentNoticesState;
use axum::{Router, routing::post};

pub(crate) const MAX_CONTENT_NOTICE_BODY_BYTES: usize = 16 * 1024;

pub(crate) fn router(state: ContentNoticesState) -> Router {
    Router::new()
        .route("/v1/content-notices", post(submit::submit))
        .with_state(state)
}

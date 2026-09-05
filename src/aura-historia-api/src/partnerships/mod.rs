mod list_admin;

use crate::state::PartnershipsState;
use axum::{Router, routing::get};

pub(crate) fn router(state: PartnershipsState) -> Router {
    Router::new()
        .route("/api/v1/admin/partnerships", get(list_admin::list_admin))
        .with_state(state)
}

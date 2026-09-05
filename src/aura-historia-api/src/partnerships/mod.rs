mod delete_member;
mod get_admin;
mod list_admin;
mod put_member;

use crate::state::PartnershipsState;
use axum::{
    Router,
    routing::{get, put},
};

pub(crate) fn router(state: PartnershipsState) -> Router {
    Router::new()
        .route("/api/v1/admin/partnerships", get(list_admin::list_admin))
        .route(
            "/api/v1/admin/partnerships/{partnership_id}",
            get(get_admin::get_admin),
        )
        .route(
            "/api/v1/admin/partnerships/{partnership_id}/members/{user_id}",
            put(put_member::put_member).delete(delete_member::delete_member),
        )
        .with_state(state)
}

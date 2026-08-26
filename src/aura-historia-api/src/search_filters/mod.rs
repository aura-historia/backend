mod create;
mod delete;
mod get;
mod list;
mod list_matches;
mod types;
mod update;
mod update_match_feedback;
mod util;

use crate::state::SearchFiltersState;
use axum::Router;
use axum::routing::{get, patch};

pub fn router(state: SearchFiltersState) -> Router {
    Router::new()
        .route(
            "/api/v1/me/search-filters",
            get(list::list_search_filters).post(create::create_search_filter),
        )
        .route(
            "/api/v1/me/search-filters/{user_search_filter_id}",
            get(get::get_search_filter)
                .patch(update::update_search_filter)
                .delete(delete::delete_search_filter),
        )
        .route(
            "/api/v1/me/search-filters/{user_search_filter_id}/matches",
            get(list_matches::list_search_filter_matches),
        )
        .route(
            "/api/v1/me/search-filters/{user_search_filter_id}/matches/{product_listing_id}",
            patch(update_match_feedback::update_search_filter_match_feedback),
        )
        .with_state(state)
}

use super::product_data::personalized_product_details_data;
use application::operation_context::Principal;
use axum::Json;
use axum::http::{HeaderMap, HeaderValue, header};
use axum::response::{IntoResponse, Response};
use product_listing_service::use_cases::PersonalizedProductListingDetailsView;

pub(crate) fn product_response(
    view: PersonalizedProductListingDetailsView,
    principal: &Principal,
    request_headers: &HeaderMap,
) -> Response {
    let ttl = crate::shipping::listing::cache_ttl([view.item.shipping.as_ref()], 120);
    let content_language = view
        .item
        .title
        .as_ref()
        .map(|title| title.localization.as_str());
    let response = Json(personalized_product_details_data(view)).into_response();
    let mut response = crate::transport::cache::anonymous_shared_success(
        response,
        request_headers,
        matches!(principal, Principal::Anonymous),
        ttl,
    );
    let headers = response.headers_mut();
    if let Some(language) = content_language {
        headers.insert(header::CONTENT_LANGUAGE, HeaderValue::from_static(language));
    }
    response
}

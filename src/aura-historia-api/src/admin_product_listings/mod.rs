mod search;

pub(crate) use crate::product_listing_management::no_store;
use crate::product_listing_management::{
    parse_body,
    types::{DetailsData, MutationData, UpdateData, VersionData, version},
};
use crate::{
    auth::protected_context, error::*, state::AdminProductListingsState, wire::parse_path_object_id,
};
use axum::{
    Json,
    body::Bytes,
    extract::{Path, RawQuery, State, rejection::BytesRejection},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use product_listing_core::product_listing_id::ProductListingId;
use product_listing_service::use_cases::{
    admin_product_listing_error::AdminProductListingError,
    commands::admin_product_listings::AdminProductListingLifecycleCommand,
};
pub use search::search_admin_product_listings;

pub async fn get_admin_product_listing(
    State(state): State<AdminProductListingsState>,
    headers: HeaderMap,
    Path(raw_id): Path<String>,
) -> Response {
    let (context, _) = match protected_context(state.authenticator.as_ref(), &headers).await {
        Ok(value) => value,
        Err(response) => return *response,
    };
    let id = match parse_id(&raw_id) {
        Ok(id) => id,
        Err(error) => return error.into_response(),
    };
    match state.get.execute(&context, id).await {
        Ok(value) => Json(DetailsData::from(value)).into_response(),
        Err(error) => ApiError::from(error).into_response(),
    }
}

pub async fn update_admin_product_listing(
    State(state): State<AdminProductListingsState>,
    headers: HeaderMap,
    Path(raw_id): Path<String>,
    body: Result<Bytes, BytesRejection>,
) -> Response {
    let (context, _) = match protected_context(state.authenticator.as_ref(), &headers).await {
        Ok(value) => value,
        Err(response) => return *response,
    };
    let command = parse_id(&raw_id).and_then(|id| parse_body::<UpdateData>(body)?.into_command(id));
    let command = match command {
        Ok(command) => command,
        Err(error) => return error.into_response(),
    };
    match state.update.execute(&context, command).await {
        Ok(value) => Json(MutationData::from(value)).into_response(),
        Err(error) => ApiError::from(error).into_response(),
    }
}

pub async fn withdraw_admin_product_listing(
    State(state): State<AdminProductListingsState>,
    headers: HeaderMap,
    Path(raw_id): Path<String>,
    RawQuery(query): RawQuery,
) -> Response {
    let (context, _) = match protected_context(state.authenticator.as_ref(), &headers).await {
        Ok(value) => value,
        Err(response) => return *response,
    };
    let command = parse_id(&raw_id).and_then(|product_listing_id| {
        let data: VersionData = crate::wire::from_query_str(query.as_deref().unwrap_or_default())
            .map_err(|_| {
            ApiError::bad_request(BAD_QUERY_PARAMETER_VALUE).with_query_field("expectedVersion")
        })?;
        let expected_version = version(data.expected_version).map_err(|_| {
            ApiError::bad_request(BAD_QUERY_PARAMETER_VALUE).with_query_field("expectedVersion")
        })?;
        Ok(AdminProductListingLifecycleCommand {
            product_listing_id,
            expected_version,
        })
    });
    let command = match command {
        Ok(command) => command,
        Err(error) => return error.into_response(),
    };
    match state.withdraw.execute(&context, command).await {
        Ok(_) => StatusCode::NO_CONTENT.into_response(),
        Err(error) => ApiError::from(error).into_response(),
    }
}

pub async fn restore_admin_product_listing(
    State(state): State<AdminProductListingsState>,
    headers: HeaderMap,
    Path(raw_id): Path<String>,
    body: Result<Bytes, BytesRejection>,
) -> Response {
    let (context, _) = match protected_context(state.authenticator.as_ref(), &headers).await {
        Ok(value) => value,
        Err(response) => return *response,
    };
    let command = parse_id(&raw_id).and_then(|product_listing_id| {
        Ok(AdminProductListingLifecycleCommand {
            product_listing_id,
            expected_version: version(parse_body::<VersionData>(body)?.expected_version)?,
        })
    });
    let command = match command {
        Ok(command) => command,
        Err(error) => return error.into_response(),
    };
    match state.restore.execute(&context, command).await {
        Ok(value) => Json(MutationData::from(value)).into_response(),
        Err(error) => ApiError::from(error).into_response(),
    }
}

fn parse_id(raw: &str) -> Result<ProductListingId, ApiError> {
    parse_path_object_id(raw, "productListingId", "ProductListing")
}
impl From<AdminProductListingError> for ApiError {
    fn from(error: AdminProductListingError) -> Self {
        match error {
            AdminProductListingError::AuthenticatedActorRequired => {
                Self::unauthorized(INVALID_CREDENTIALS)
            }
            AdminProductListingError::Forbidden => Self::forbidden(FORBIDDEN),
            AdminProductListingError::NotFound => Self::not_found(PRODUCT_LISTING_NOT_FOUND),
            AdminProductListingError::ConcurrencyConflict => {
                Self::conflict(PRODUCT_LISTING_CONCURRENCY_CONFLICT)
            }
            AdminProductListingError::ListingWithdrawn => Self::conflict(PRODUCT_LISTING_WITHDRAWN),
            AdminProductListingError::InvalidPatch { .. } => Self::bad_request(BAD_BODY_VALUE),
            AdminProductListingError::AuctionNotFound => {
                Self::not_found(AUCTION_NOT_FOUND).with_body_field("auction.auctionId")
            }
            AdminProductListingError::AuctionSourceMismatch => {
                Self::conflict(CONFLICT).with_body_field("auction.auctionId")
            }
            AdminProductListingError::InvalidPageSize => {
                Self::bad_request(BAD_QUERY_PARAMETER_VALUE).with_query_field("size")
            }
            AdminProductListingError::InvalidQuery => {
                Self::bad_request(BAD_QUERY_PARAMETER_VALUE).with_query_field("query")
            }
            AdminProductListingError::CursorScopeMismatch => {
                Self::bad_request(BAD_QUERY_PARAMETER_VALUE).with_query_field("searchAfter")
            }
            AdminProductListingError::TemporarilyUnavailable { .. }
            | AdminProductListingError::BeginTransactionFailed(_)
            | AdminProductListingError::CommitTransactionFailed(_) => {
                Self::service_unavailable(PRODUCT_LISTING_TEMPORARILY_UNAVAILABLE)
            }
            AdminProductListingError::InvalidPersistedState { .. }
            | AdminProductListingError::Internal { .. } => {
                Self::internal_server_error(PRODUCT_LISTING_INTERNAL_ERROR)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use application::error::static_error;
    #[test]
    fn body_errors_identify_fields_and_reject_trailing_json() {
        for (body, field) in [
            (
                serde_json::json!({"expectedVersion":null}),
                "expectedVersion",
            ),
            (
                serde_json::json!({"expectedVersion":1,"availability":"SOLD"}),
                "availability",
            ),
            (
                serde_json::json!({"expectedVersion":1,"images":[null]}),
                "images",
            ),
            (
                serde_json::json!({"expectedVersion":1,"auction":{"timing":{"biddingOpens":"invalid"}}}),
                "auction.timing.biddingOpens",
            ),
        ] {
            let error = parse_body::<UpdateData>(Ok(Bytes::from(body.to_string()))).unwrap_err();
            let error = serde_json::to_value(error).unwrap();
            assert_eq!(field, error["source"]["field"]);
            assert_eq!("BODY", error["source"]["type"]);
        }
        assert!(
            parse_body::<VersionData>(Ok(Bytes::from_static(b"{\"expectedVersion\":1} {}")))
                .is_err()
        );
    }

    #[test]
    fn maps_typed_failures_without_exposing_technical_details() {
        for (error, expected) in [
            (
                AdminProductListingError::ConcurrencyConflict,
                StatusCode::CONFLICT,
            ),
            (
                AdminProductListingError::ListingWithdrawn,
                StatusCode::CONFLICT,
            ),
            (
                AdminProductListingError::TemporarilyUnavailable {
                    source: static_error("private technical cause"),
                },
                StatusCode::SERVICE_UNAVAILABLE,
            ),
            (
                AdminProductListingError::InvalidPersistedState {
                    source: static_error("private technical cause"),
                },
                StatusCode::INTERNAL_SERVER_ERROR,
            ),
            (
                AdminProductListingError::Internal {
                    source: static_error("private technical cause"),
                },
                StatusCode::INTERNAL_SERVER_ERROR,
            ),
            (
                AdminProductListingError::AuctionNotFound,
                StatusCode::NOT_FOUND,
            ),
            (
                AdminProductListingError::AuctionSourceMismatch,
                StatusCode::CONFLICT,
            ),
        ] {
            let error = ApiError::from(error);
            assert!(
                !serde_json::to_string(&error)
                    .unwrap()
                    .contains("private technical cause")
            );
            assert_eq!(expected, error.into_response().status());
        }
    }
}

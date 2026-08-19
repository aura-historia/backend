pub mod actor;
pub mod currency;
pub mod distance;

#[cfg(feature = "api")]
pub mod api;
pub mod batch;
// Legacy shim. Owner: domain-primitives. Remove after legacy common consumers migrate.
pub mod change_outcome {
    pub type ChangeOutcome = domain_primitives::change_outcome::ChangeOutcome;
}
pub mod domain;
pub mod enhanced_match_reason;

#[cfg(feature = "dynamodb")]
pub mod dynamodb_update;

#[cfg(feature = "event_bridge")]
pub mod dynamodb_stream;

pub mod error;
// Legacy shim. Owner: domain-primitives. Remove after legacy common consumers migrate.
pub mod event {
    pub type Event<AggregateId, Payload> = domain_primitives::event::Event<AggregateId, Payload>;
}

// Legacy shim. Owner: domain-primitives. The API extraction module remains legacy-only.
pub mod event_id {
    pub type EventId = domain_primitives::event_id::EventId;

    #[cfg(feature = "api")]
    pub mod api {
        use crate::{
            api::{
                error::ApiError,
                error_code::{BAD_PAGE_SIZE_VALUE, BAD_PATH_PARAMETER_VALUE, INVALID_UUID},
            },
            event_id::EventId,
            pagination::cursor::Cursor,
        };
        use aws_lambda_events::query_map::QueryMap;
        use std::collections::HashMap;

        pub fn extract_event_id_path(
            path_params: &HashMap<String, String>,
        ) -> Result<EventId, ApiError> {
            path_params
                .get("eventId")
                .map(|value| EventId::try_from(value.as_str()))
                .transpose()
                .map_err(|error| {
                    let detail = error.to_string();
                    ApiError::bad_request(INVALID_UUID, Box::new(error))
                        .with_path_field("eventId")
                        .with_detail(detail)
                })?
                .ok_or(
                    ApiError::bad_request(
                        BAD_PATH_PARAMETER_VALUE,
                        "Missing path parameter 'eventId'.".into(),
                    )
                    .with_path_field("eventId")
                    .with_detail("Missing field 'eventId'."),
                )
        }

        pub fn extract_event_id_cursor_query(
            query: &QueryMap,
        ) -> Result<Option<Cursor<EventId>>, ApiError> {
            let search_after = query
                .first("searchAfter")
                .map(str::trim)
                .map(EventId::try_from)
                .transpose()
                .map_err(|error| {
                    let detail = error.to_string();
                    ApiError::bad_request(INVALID_UUID, Box::new(error))
                        .with_query_field("searchAfter")
                        .with_detail(detail)
                })?;
            let size = query
                .first("size")
                .map(str::trim)
                .map(str::parse::<u64>)
                .transpose()
                .map_err(|error| {
                    let detail = error.to_string();
                    ApiError::bad_request(BAD_PAGE_SIZE_VALUE, Box::new(error))
                        .with_query_field("size")
                        .with_detail(detail)
                })?
                .map(|size| size.min(100));

            Ok(size.map(|size| Cursor { search_after, size }))
        }
    }
}
pub mod execution_state;
pub mod fx_rate_id;

#[cfg(feature = "test-data")]
pub mod fake;
pub mod has_key;
pub mod language;
pub mod localized;
pub mod logging;
pub mod measurement_unit;
pub mod mergeable;
pub mod product_id;
pub mod product_lifecycle;
pub mod product_slug_id;
pub mod product_state;

pub mod oauth_client_id;
#[cfg(feature = "opensearch")]
pub mod opensearch;
pub mod operation_context;
pub mod pagination;
pub mod partner_shop_application_id;
pub mod patch_field;
pub mod personalized;
#[cfg(feature = "postgres")]
pub mod postgres;
pub mod price;
pub mod query;
pub mod resource_state;
pub mod seller_slug_id;
pub mod serde;
pub mod shop_id;
pub mod shop_name;
pub mod shop_slug_id;
pub mod shops_product_id;
pub mod slug_id;
pub mod sort;
pub mod string_newtype;
pub mod stripe_customer_id;
pub mod transaction;
pub mod user_id;
pub mod user_search_filter_id;
pub mod user_search_filter_name;
pub mod utm;
pub mod uuid_newtype;
pub mod version;
pub mod versioned;
pub mod year;

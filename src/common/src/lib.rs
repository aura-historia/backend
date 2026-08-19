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
// Legacy shim. Owner: shop-core. Remove after legacy common consumers migrate.
pub mod domain {
    pub type Domain = shop_core::domain::Domain;
    pub type NoDomainError = shop_core::domain::NoDomainError;
}
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
// Legacy shim. Owner: shop-core. Remove after legacy common consumers migrate.
pub mod seller_slug_id {
    pub type SellerSlugId = shop_core::seller_slug_id::SellerSlugId;

    impl From<crate::slug_id::SlugId<0>> for SellerSlugId {
        fn from(value: crate::slug_id::SlugId<0>) -> Self {
            Self::from(value.to_string())
        }
    }

    impl From<SellerSlugId> for crate::slug_id::SlugId<0> {
        fn from(value: SellerSlugId) -> Self {
            Self::from(value.to_string())
        }
    }
}
pub mod serde;
// Legacy shim. Owner: shop-core. Remove after legacy common consumers migrate.
pub mod shop_id {
    pub type ShopId = shop_core::shop_id::ShopId;

    #[cfg(feature = "api")]
    pub mod api {
        use crate::{
            api::{
                error::ApiError,
                error_code::{BAD_PATH_PARAMETER_VALUE, INVALID_UUID},
            },
            error::missing_field::MissingRequiredField,
            shop_id::ShopId,
            shop_slug_id::ShopSlugId,
        };
        use std::collections::HashMap;

        pub fn extract_shop_id_path(
            path_params: &HashMap<String, String>,
        ) -> Result<ShopId, ApiError> {
            path_params
                .get("shopId")
                .map(ShopId::try_from)
                .transpose()
                .map_err(|error| {
                    let detail = error.to_string();
                    ApiError::bad_request(INVALID_UUID, Box::new(error))
                        .with_path_field("shopId")
                        .with_detail(detail)
                })?
                .ok_or(
                    ApiError::bad_request(
                        BAD_PATH_PARAMETER_VALUE,
                        Box::new(MissingRequiredField::new("shopId")),
                    )
                    .with_path_field("shopId")
                    .with_detail("Missing field 'shopId'."),
                )
        }

        pub fn extract_shop_slug_id_path(
            path_params: &HashMap<String, String>,
        ) -> Result<ShopSlugId, ApiError> {
            path_params
                .get("shopSlugId")
                .map(ShopSlugId::raw)
                .transpose()
                .map_err(|error| {
                    let detail = error.to_string();
                    ApiError::bad_request(BAD_PATH_PARAMETER_VALUE, Box::new(error))
                        .with_path_field("shopSlugId")
                        .with_detail(detail)
                })?
                .ok_or(
                    ApiError::bad_request(
                        BAD_PATH_PARAMETER_VALUE,
                        Box::new(MissingRequiredField::new("shopSlugId")),
                    )
                    .with_path_field("shopSlugId")
                    .with_detail("Missing field 'shopSlugId'."),
                )
        }
    }
}
// Legacy shim. Owner: shop-core. Remove after legacy common consumers migrate.
pub mod shop_name {
    pub type ShopName = shop_core::shop_name::ShopName;
}
// Legacy shim. Owner: shop-core. Remove after legacy common consumers migrate.
pub mod shop_slug_id {
    pub type InvalidShopSlugId = shop_core::shop_slug_id::InvalidShopSlugId;
    pub type ShopSlugId = shop_core::shop_slug_id::ShopSlugId;

    impl From<crate::slug_id::SlugId<0>> for ShopSlugId {
        fn from(value: crate::slug_id::SlugId<0>) -> Self {
            Self::from(value.to_string())
        }
    }

    impl From<ShopSlugId> for crate::slug_id::SlugId<0> {
        fn from(value: ShopSlugId) -> Self {
            Self::from(value.to_string())
        }
    }
}
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

use crate::product_listing_management::{
    parse_body,
    types::{DetailsData, MutationData, UpdateData, VersionData, version},
};
use crate::{
    auth::protected_context, error::*, state::PartnerProductListingManagementState,
    wire::parse_path_object_id,
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
    commands::partner_product_listings::{
        PartnerProductListingLifecycleCommand, UpdatePartnerProductListingCommand,
    },
    partner_product_listing_error::PartnerProductListingError,
};

pub async fn get_partner_product_listing(
    State(state): State<PartnerProductListingManagementState>,
    headers: HeaderMap,
    Path((raw_source, raw_id)): Path<(String, String)>,
) -> Response {
    let (context, _) = match protected_context(state.authenticator.as_ref(), &headers).await {
        Ok(value) => value,
        Err(response) => return *response,
    };
    let (source, id) = match parse_ids(&raw_source, &raw_id) {
        Ok(id) => id,
        Err(error) => return error.into_response(),
    };
    match state.get.execute(&context, source, id).await {
        Ok(value) => Json(DetailsData::from(value)).into_response(),
        Err(error) => ApiError::from(error).into_response(),
    }
}

pub async fn update_partner_product_listing(
    State(state): State<PartnerProductListingManagementState>,
    headers: HeaderMap,
    Path((raw_source, raw_id)): Path<(String, String)>,
    body: Result<Bytes, BytesRejection>,
) -> Response {
    let (context, _) =
        match crate::auth::command_context(state.authenticator.as_ref(), &headers).await {
            Ok(value) => value,
            Err(response) => return *response,
        };
    let command = parse_ids(&raw_source, &raw_id).and_then(|(listing_source_id, id)| {
        let command = parse_body::<UpdateData>(body)?.into_command(id)?;
        Ok(UpdatePartnerProductListingCommand {
            listing_source_id,
            product_listing_id: id,
            expected_version: command.expected_version,
            patch: command.patch,
        })
    });
    let command = match command {
        Ok(command) => command,
        Err(error) => return error.into_response(),
    };
    match state.update.execute(&context, command).await {
        Ok(value) => Json(MutationData::from(value)).into_response(),
        Err(error) => ApiError::from(error).into_response(),
    }
}

pub async fn withdraw_partner_product_listing(
    State(state): State<PartnerProductListingManagementState>,
    headers: HeaderMap,
    Path((raw_source, raw_id)): Path<(String, String)>,
    RawQuery(query): RawQuery,
) -> Response {
    let (context, _) =
        match crate::auth::command_context(state.authenticator.as_ref(), &headers).await {
            Ok(value) => value,
            Err(response) => return *response,
        };
    let command =
        parse_ids(&raw_source, &raw_id).and_then(|(listing_source_id, product_listing_id)| {
            let data: VersionData = serde_qs::from_str(query.as_deref().unwrap_or_default())
                .map_err(|_| {
                    ApiError::bad_request(BAD_QUERY_PARAMETER_VALUE)
                        .with_query_field("expectedVersion")
                })?;
            let expected_version = version(data.expected_version).map_err(|_| {
                ApiError::bad_request(BAD_QUERY_PARAMETER_VALUE).with_query_field("expectedVersion")
            })?;
            Ok(PartnerProductListingLifecycleCommand {
                listing_source_id,
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

pub async fn restore_partner_product_listing(
    State(state): State<PartnerProductListingManagementState>,
    headers: HeaderMap,
    Path((raw_source, raw_id)): Path<(String, String)>,
    body: Result<Bytes, BytesRejection>,
) -> Response {
    let (context, _) =
        match crate::auth::command_context(state.authenticator.as_ref(), &headers).await {
            Ok(value) => value,
            Err(response) => return *response,
        };
    let command =
        parse_ids(&raw_source, &raw_id).and_then(|(listing_source_id, product_listing_id)| {
            Ok(PartnerProductListingLifecycleCommand {
                listing_source_id,
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

fn parse_ids(
    source: &str,
    listing: &str,
) -> Result<(listing_source_core::ListingSourceId, ProductListingId), ApiError> {
    Ok((
        parse_path_object_id(source, "listingSourceId", "ListingSource")?,
        parse_path_object_id(listing, "productListingId", "ProductListing")?,
    ))
}

pub async fn list_partner_product_listings(
    State(state): State<PartnerProductListingManagementState>,
    headers: HeaderMap,
    Path(raw_source): Path<String>,
    RawQuery(raw): RawQuery,
) -> Response {
    use crate::product_listing_management::search::{SearchQuery, SearchResponse};
    let (context, _) = match protected_context(state.authenticator.as_ref(), &headers).await {
        Ok(value) => value,
        Err(response) => return *response,
    };
    let request =
        parse_path_object_id(&raw_source, "listingSourceId", "ListingSource").and_then(|source| {
            let query: SearchQuery = serde_qs::from_str(raw.as_deref().unwrap_or_default())
                .map_err(|_| {
                    ApiError::bad_request(BAD_QUERY_PARAMETER_VALUE).with_query_field("query")
                })?;
            Ok((source, query.into_partner_request(source)?))
        });
    let (source, request) = match request {
        Ok(value) => value,
        Err(error) => return error.into_response(),
    };
    match state.list.execute(&context, source, request).await {
        Ok(value) => Json(SearchResponse::from(value)).into_response(),
        Err(error) => ApiError::from(error).into_response(),
    }
}

impl From<PartnerProductListingError> for ApiError {
    fn from(error: PartnerProductListingError) -> Self {
        match error {
            PartnerProductListingError::AuthenticatedActorRequired => {
                Self::unauthorized(INVALID_CREDENTIALS)
            }
            PartnerProductListingError::Forbidden => Self::forbidden(FORBIDDEN),
            PartnerProductListingError::Management(error) => error.into(),
        }
    }
}

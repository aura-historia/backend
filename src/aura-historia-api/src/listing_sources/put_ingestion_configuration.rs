use crate::auth::protected_context;
use crate::error::{ApiError, BAD_BODY_VALUE, LISTING_SOURCE_INTERNAL_ERROR};
use crate::state::ListingSourcesState;
use crate::wire::parse_path_object_id;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use listing_source_core::{Domain, ListingSourceId, WoocommerceWebhookSecret};
use listing_source_service::ports::ListingIngestionConfiguration;
use partnership_service::use_cases::commands::put_listing_source_ingestion_configuration::PutListingSourceIngestionConfigurationCommand;
use serde::Deserialize;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PutWoocommerceConfigurationData {
    webhook_secret: String,
    currency: Option<String>,
    language: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PutShopifyConfigurationData {
    domain: String,
    currency: Option<String>,
    language: Option<String>,
}

pub async fn put_woocommerce_configuration(
    State(state): State<ListingSourcesState>,
    headers: HeaderMap,
    Path(raw_listing_source_id): Path<String>,
    body: String,
) -> Response {
    let listing_source_id = match parse_id(&raw_listing_source_id) {
        Ok(value) => value,
        Err(error) => return no_store(error.into_response()),
    };
    let (context, _) = match protected_context(state.authenticator.as_ref(), &headers).await {
        Ok(value) => value,
        Err(response) => return no_store(*response),
    };
    let data = match parse_body::<PutWoocommerceConfigurationData>(&body) {
        Ok(value) => value,
        Err(error) => return no_store(error.into_response()),
    };
    let webhook_secret = match WoocommerceWebhookSecret::try_from(data.webhook_secret) {
        Ok(value) => value,
        Err(_) => return no_store(invalid_field("webhookSecret").into_response()),
    };
    let (currency, language) = match provider_values(data.currency, data.language) {
        Ok(value) => value,
        Err(error) => return no_store(error.into_response()),
    };
    no_store(
        put_configuration(
            &state,
            &context,
            listing_source_id,
            ListingIngestionConfiguration::Woocommerce {
                webhook_secret,
                currency,
                language,
            },
        )
        .await,
    )
}

pub async fn put_shopify_configuration(
    State(state): State<ListingSourcesState>,
    headers: HeaderMap,
    Path(raw_listing_source_id): Path<String>,
    body: String,
) -> Response {
    let listing_source_id = match parse_id(&raw_listing_source_id) {
        Ok(value) => value,
        Err(error) => return no_store(error.into_response()),
    };
    let (context, _) = match protected_context(state.authenticator.as_ref(), &headers).await {
        Ok(value) => value,
        Err(response) => return no_store(*response),
    };
    let data = match parse_body::<PutShopifyConfigurationData>(&body) {
        Ok(value) => value,
        Err(error) => return no_store(error.into_response()),
    };
    let domain = match Domain::try_from(data.domain) {
        Ok(value) => value,
        Err(_) => return no_store(invalid_field("domain").into_response()),
    };
    let (currency, language) = match provider_values(data.currency, data.language) {
        Ok(value) => value,
        Err(error) => return no_store(error.into_response()),
    };
    no_store(
        put_configuration(
            &state,
            &context,
            listing_source_id,
            ListingIngestionConfiguration::Shopify {
                domain,
                currency,
                language,
            },
        )
        .await,
    )
}

async fn put_configuration(
    state: &ListingSourcesState,
    context: &application::operation_context::OperationContext,
    listing_source_id: ListingSourceId,
    configuration: ListingIngestionConfiguration,
) -> Response {
    let Some(put_ingestion_configuration) = state.put_ingestion_configuration.as_ref() else {
        return ApiError::internal_server_error(LISTING_SOURCE_INTERNAL_ERROR)
            .with_detail("Listing source configuration is not available.")
            .into_response();
    };
    match put_ingestion_configuration
        .execute(
            context,
            PutListingSourceIngestionConfigurationCommand {
                listing_source_id,
                configuration,
            },
        )
        .await
    {
        Ok(result) => {
            let status = if result.created {
                StatusCode::CREATED
            } else {
                StatusCode::NO_CONTENT
            };
            status.into_response()
        }
        Err(error) => ApiError::from(error).into_response(),
    }
}

fn no_store(mut response: Response) -> Response {
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

fn parse_id(value: &str) -> Result<ListingSourceId, ApiError> {
    parse_path_object_id(value, "listingSourceId", "ListingSource")
}

fn parse_body<T: for<'de> Deserialize<'de>>(body: &str) -> Result<T, ApiError> {
    if body.trim().is_empty() {
        return Err(ApiError::bad_request(BAD_BODY_VALUE).with_detail("Body cannot be empty."));
    }
    serde_json::from_str(body)
        .map_err(|error| ApiError::bad_request(BAD_BODY_VALUE).with_detail(error.to_string()))
}

fn provider_values(
    currency: Option<String>,
    language: Option<String>,
) -> Result<(Option<money::Currency>, Option<localization::Language>), ApiError> {
    let currency = currency
        .map(|value| money::Currency::from_code(&value).ok_or_else(|| invalid_field("currency")))
        .transpose()?;
    let language = language
        .map(|value| {
            localization::Language::from_code(&value).ok_or_else(|| invalid_field("language"))
        })
        .transpose()?;
    Ok((currency, language))
}

fn invalid_field(field: &str) -> ApiError {
    ApiError::bad_request(BAD_BODY_VALUE).with_detail(format!("Body field '{field}' is invalid."))
}

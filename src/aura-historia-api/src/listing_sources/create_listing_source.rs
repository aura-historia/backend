use crate::auth::protected_context;
use crate::error::{ApiError, BAD_BODY_VALUE};
use crate::listing_sources::types::{CreateListingSourceData, ListingSourceReferenceData};
use crate::state::ListingSourcesState;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use listing_source_core::ListingSourcePresentation;
use listing_source_service::use_cases::commands::create_listing_source::{
    CreateListingSourceCommand, ListingSourceOperator,
};

pub async fn create_listing_source(
    State(state): State<ListingSourcesState>,
    headers: HeaderMap,
    body: String,
) -> Response {
    let (context, _) = match protected_context(state.authenticator.as_ref(), &headers).await {
        Ok(value) => value,
        Err(response) => return *response,
    };
    let data = match parse_body(&body) {
        Ok(data) => data,
        Err(error) => return error.into_response(),
    };
    let acquisition_configuration = match data
        .acquisition_configuration
        .into_iter()
        .map(TryInto::try_into)
        .collect::<Result<Vec<_>, ApiError>>()
    {
        Ok(value) => listing_source_service::ports::ListingSourceAcquisitionConfigurations(value),
        Err(error) => return error.into_response(),
    };
    let command = CreateListingSourceCommand {
        name: data.name.into(),
        operator: ListingSourceOperator::Existing(data.operator_party_id.into()),
        acquisition_methods: data.acquisition_methods,
        acquisition_configuration,
        woocommerce_webhook_secret: data.woocommerce_webhook_secret,
        presentation: ListingSourcePresentation {
            url: data.url,
            image: data.image,
        },
        referral_configuration: data.referral_configuration.map(Into::into),
    };

    match state.create.execute(&context, command).await {
        Ok(result) => {
            let mut response = axum::Json(ListingSourceReferenceData::from((
                result.listing_source_id,
                result.slug_id,
            )))
            .into_response();
            *response.status_mut() = StatusCode::CREATED;
            response
        }
        Err(error) => ApiError::from(error).into_response(),
    }
}

fn parse_body(body: &str) -> Result<CreateListingSourceData, ApiError> {
    if body.trim().is_empty() {
        return Err(ApiError::bad_request(BAD_BODY_VALUE).with_detail("Body cannot be empty."));
    }
    serde_json::from_str(body)
        .map_err(|error| ApiError::bad_request(BAD_BODY_VALUE).with_detail(error.to_string()))
}

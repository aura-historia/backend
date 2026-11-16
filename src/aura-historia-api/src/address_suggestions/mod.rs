//! `POST /v1/address-suggestions`: search-as-you-type address suggestions for signed-in
//! people. Results may not be stored or cached, so every response is `no-store`.
use crate::{
    auth::protected_context,
    error::{
        ADDRESS_SUGGESTIONS_INTERNAL_ERROR, ADDRESS_SUGGESTIONS_RATE_LIMITED,
        ADDRESS_SUGGESTIONS_UNAVAILABLE, ApiError, BAD_BODY_VALUE, FORBIDDEN, INVALID_CREDENTIALS,
    },
    state::AddressSuggestionsState,
};
use aura_historia_contracts::address_suggestions::{SuggestData, SuggestionsData};
use axum::{
    Json,
    extract::State,
    http::HeaderMap,
    response::{IntoResponse, Response},
};
use geo_service::suggestions::SuggestAddressesError;

fn bad() -> ApiError {
    ApiError::bad_request(BAD_BODY_VALUE).with_detail("Invalid address suggestion input.")
}

impl From<SuggestAddressesError> for ApiError {
    fn from(e: SuggestAddressesError) -> Self {
        match e {
            SuggestAddressesError::AuthenticationRequired => {
                Self::unauthorized(INVALID_CREDENTIALS)
            }
            SuggestAddressesError::Forbidden => Self::forbidden(FORBIDDEN),
            SuggestAddressesError::RateLimited(retry_after) => {
                Self::too_many_requests(ADDRESS_SUGGESTIONS_RATE_LIMITED, retry_after.seconds())
                    .with_detail("Too many address suggestion requests. Try again later.")
            }
            error @ (SuggestAddressesError::Unavailable(_)
            | SuggestAddressesError::Storage(_)
            | SuggestAddressesError::Transaction(_)) => {
                Self::service_unavailable(ADDRESS_SUGGESTIONS_UNAVAILABLE)
                    .with_detail("Address suggestions are unavailable right now.")
                    .with_cause(Box::new(error))
            }
            error @ SuggestAddressesError::Provider(_) => {
                Self::internal_server_error(ADDRESS_SUGGESTIONS_INTERNAL_ERROR)
                    .with_cause(Box::new(error))
            }
        }
    }
}

pub(crate) async fn suggest(
    State(s): State<AddressSuggestionsState>,
    headers: HeaderMap,
    body: String,
) -> Response {
    let (ctx, _) = match protected_context(s.authenticator.as_ref(), &headers).await {
        Ok(v) => v,
        Err(r) => return *r,
    };
    let request = serde_json::from_str::<SuggestData>(&body)
        .map_err(|_| bad())
        .and_then(|d| d.into_request().map_err(|_| bad()));
    let (query, language) = match request {
        Ok(v) => v,
        Err(e) => return e.into_response(),
    };
    match s.suggest.execute(&ctx, query, language).await {
        Ok(v) => Json(SuggestionsData::from(v)).into_response(),
        Err(e) => ApiError::from(e).into_response(),
    }
}

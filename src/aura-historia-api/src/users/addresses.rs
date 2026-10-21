#[cfg(test)]
use aura_historia_contracts::users::destination::DestinationData;
use aura_historia_contracts::users::{
    addresses::{AddressData, CreateData, ExpectedData, PageData, PageQuery, UpdateData},
    destination::{DestinationResponse, SetDestinationData},
};

use crate::{
    auth::protected_context,
    error::{
        ADDRESS_BOOK_INTERNAL_ERROR, ADDRESS_BOOK_TEMPORARILY_UNAVAILABLE, ApiError,
        BAD_BODY_VALUE, BAD_QUERY_PARAMETER_VALUE, CONFLICT, FORBIDDEN, INVALID_CREDENTIALS,
        USER_ADDRESS_NOT_FOUND,
    },
    state::UserAddressesState,
    wire::parse_path_object_id,
};
use axum::{
    Json,
    extract::{Path, Query, State, rejection::QueryRejection},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use geo_service::resolution::ResolutionError;
use user_core::user_address::*;
use user_service::address_book::*;

fn bad() -> ApiError {
    ApiError::bad_request(BAD_BODY_VALUE).with_detail("Invalid address book input.")
}
impl From<aura_historia_contracts::users::destination::InvalidDestinationData> for ApiError {
    fn from(_: aura_historia_contracts::users::destination::InvalidDestinationData) -> Self {
        bad()
    }
}
fn parse<T: serde::de::DeserializeOwned>(body: &str) -> Result<T, ApiError> {
    serde_json::from_str(body).map_err(|_| bad())
}
impl From<AddressBookError> for ApiError {
    fn from(e: AddressBookError) -> Self {
        match e {
            AddressBookError::AuthenticationRequired => Self::unauthorized(INVALID_CREDENTIALS),
            AddressBookError::Forbidden
            | AddressBookError::Resolution(ResolutionError::Forbidden) => {
                Self::forbidden(FORBIDDEN)
            }
            AddressBookError::NotFound | AddressBookError::Resolution(ResolutionError::Missing) => {
                Self::not_found(USER_ADDRESS_NOT_FOUND)
            }
            AddressBookError::Conflict
            | AddressBookError::Resolution(ResolutionError::Conflict) => Self::conflict(CONFLICT),
            AddressBookError::InvalidInput
            | AddressBookError::Resolution(ResolutionError::InvalidInput) => bad(),
            error @ (AddressBookError::Storage(_)
            | AddressBookError::Transaction(_)
            | AddressBookError::Resolution(
                ResolutionError::Storage(_) | ResolutionError::Transaction(_),
            )) => Self::service_unavailable(ADDRESS_BOOK_TEMPORARILY_UNAVAILABLE)
                .with_cause(Box::new(error)),
            error => {
                Self::internal_server_error(ADDRESS_BOOK_INTERNAL_ERROR).with_cause(Box::new(error))
            }
        }
    }
}
pub(crate) async fn create(
    State(s): State<UserAddressesState>,
    headers: HeaderMap,
    body: String,
) -> Response {
    let (ctx, _) = match protected_context(s.authenticator.as_ref(), &headers).await {
        Ok(v) => v,
        Err(r) => return *r,
    };
    let command = (|| {
        let d: CreateData = parse(&body)?;
        d.into_command().map_err(ApiError::from)
    })();
    let command = match command {
        Ok(v) => v,
        Err(e) => return e.into_response(),
    };
    match s.commands.create(&ctx, command).await {
        Ok(v) => (StatusCode::CREATED, Json(AddressData::from(v))).into_response(),
        Err(e) => ApiError::from(e).into_response(),
    }
}
pub(crate) async fn get(
    State(s): State<UserAddressesState>,
    headers: HeaderMap,
    Path(raw): Path<String>,
) -> Response {
    let (ctx, _) = match protected_context(s.authenticator.as_ref(), &headers).await {
        Ok(v) => v,
        Err(r) => return *r,
    };
    let id = match parse_path_object_id::<UserAddressId>(&raw, "address_id", "UserAddress") {
        Ok(v) => v,
        Err(e) => return e.into_response(),
    };
    match s.queries.get(&ctx, id).await {
        Ok(v) => Json(AddressData::from(v)).into_response(),
        Err(e) => ApiError::from(e).into_response(),
    }
}
pub(crate) async fn list(
    State(s): State<UserAddressesState>,
    headers: HeaderMap,
    query: Result<Query<PageQuery>, QueryRejection>,
) -> Response {
    let (ctx, _) = match protected_context(s.authenticator.as_ref(), &headers).await {
        Ok(v) => v,
        Err(r) => return *r,
    };
    let Query(q) = match query {
        Ok(v) => v,
        Err(_) => return ApiError::bad_request(BAD_QUERY_PARAMETER_VALUE).into_response(),
    };
    match s.queries.list(&ctx, q.after, q.limit.unwrap_or(25)).await {
        Ok(v) => Json(PageData::from(v)).into_response(),
        Err(e) => ApiError::from(e).into_response(),
    }
}
pub(crate) async fn update(
    State(s): State<UserAddressesState>,
    headers: HeaderMap,
    Path(raw): Path<String>,
    body: String,
) -> Response {
    let (ctx, _) = match protected_context(s.authenticator.as_ref(), &headers).await {
        Ok(v) => v,
        Err(r) => return *r,
    };
    let command = (|| {
        let id = parse_path_object_id(&raw, "address_id", "UserAddress")?;
        let d: UpdateData = parse(&body)?;
        d.into_command(id).map_err(ApiError::from)
    })();
    let c = match command {
        Ok(v) => v,
        Err(e) => return e.into_response(),
    };
    match s.commands.update(&ctx, c).await {
        Ok(v) => Json(AddressData::from(v)).into_response(),
        Err(e) => ApiError::from(e).into_response(),
    }
}
pub(crate) async fn delete(
    State(s): State<UserAddressesState>,
    headers: HeaderMap,
    Path(raw): Path<String>,
    body: String,
) -> Response {
    let (ctx, _) = match protected_context(s.authenticator.as_ref(), &headers).await {
        Ok(v) => v,
        Err(r) => return *r,
    };
    let input = (|| {
        Ok::<_, ApiError>((
            parse_path_object_id(&raw, "address_id", "UserAddress")?,
            parse::<ExpectedData>(&body)?.expected_version,
        ))
    })();
    let (id, expected) = match input {
        Ok(v) => v,
        Err(e) => return e.into_response(),
    };
    match s.commands.delete(&ctx, id, expected).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => ApiError::from(e).into_response(),
    }
}
pub(crate) async fn get_destination(
    State(s): State<UserAddressesState>,
    headers: HeaderMap,
) -> Response {
    let (ctx, _) = match protected_context(s.authenticator.as_ref(), &headers).await {
        Ok(v) => v,
        Err(r) => return *r,
    };
    match s.queries.destination(&ctx).await {
        Ok(v) => Json(DestinationResponse::from(v)).into_response(),
        Err(e) => ApiError::from(e).into_response(),
    }
}
pub(crate) async fn set_destination(
    State(s): State<UserAddressesState>,
    headers: HeaderMap,
    body: String,
) -> Response {
    let (ctx, _) = match protected_context(s.authenticator.as_ref(), &headers).await {
        Ok(v) => v,
        Err(r) => return *r,
    };
    let input = (|| {
        let d: SetDestinationData = parse(&body)?;
        Ok::<_, ApiError>((d.destination.into_core()?, d.expected_version))
    })();
    let (destination, expected) = match input {
        Ok(v) => v,
        Err(e) => return e.into_response(),
    };
    match s
        .commands
        .set_destination(&ctx, destination, expected)
        .await
    {
        Ok(v) => Json(DestinationResponse::from(v)).into_response(),
        Err(e) => ApiError::from(e).into_response(),
    }
}
impl From<aura_historia_contracts::users::addresses::InvalidAddressData> for ApiError {
    fn from(error: aura_historia_contracts::users::addresses::InvalidAddressData) -> Self {
        use aura_historia_contracts::users::addresses::InvalidAddressData;
        match error {
            InvalidAddressData::Label(_) => bad(),
            InvalidAddressData::Geography(error) => error.into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn browsing_destination_rejects_members_outside_its_kind() {
        for data in [
            json!({"kind":"NONE","country":"FR"}),
            json!({"kind":"NONE","postalCode":"75001"}),
            json!({"kind":"NONE","addressId":"00000000-0000-0000-0000-000000000000"}),
            json!({"kind":"COUNTRY_POSTAL","country":"DE","addressId":"00000000-0000-0000-0000-000000000000"}),
        ] {
            assert!(
                serde_json::from_value::<DestinationData>(data.clone()).is_err(),
                "{data}"
            );
        }
        let none: DestinationData = serde_json::from_value(json!({"kind":"NONE"})).unwrap();
        assert!(matches!(none.into_core(), Ok(BrowsingDestination::None)));
        assert_eq!(
            json!({"kind":"NONE"}),
            serde_json::to_value(DestinationData::from(BrowsingDestination::None)).unwrap()
        );
    }
}

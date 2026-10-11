use crate::{
    auth::protected_context,
    error::{
        ADDRESS_BOOK_INTERNAL_ERROR, ADDRESS_BOOK_TEMPORARILY_UNAVAILABLE, ApiError,
        BAD_BODY_VALUE, BAD_QUERY_PARAMETER_VALUE, CONFLICT, FORBIDDEN, INVALID_CREDENTIALS,
        USER_ADDRESS_NOT_FOUND,
    },
    parties::locations::{GeographyData, PositionData},
    state::UserAddressesState,
    wire::parse_path_object_id,
};
use axum::{
    Json,
    extract::{Path, Query, State, rejection::QueryRejection},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use geo::{DerivedGeography, PostalCode, country_from_code, reference::ReferenceRelease};
use geo_service::resolution::ResolutionError;
use serde::{Deserialize, Serialize};
use user_core::user_address::*;
use user_service::address_book::*;

fn bad() -> ApiError {
    ApiError::bad_request(BAD_BODY_VALUE).with_detail("Invalid address book input.")
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
            AddressBookError::Storage(_)
            | AddressBookError::Transaction(_)
            | AddressBookError::Resolution(
                ResolutionError::Storage(_) | ResolutionError::Transaction(_),
            ) => Self::service_unavailable(ADDRESS_BOOK_TEMPORARILY_UNAVAILABLE),
            _ => Self::internal_server_error(ADDRESS_BOOK_INTERNAL_ERROR),
        }
    }
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CreateData {
    address_id: UserAddressId,
    label: String,
    geography: GeographyData,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct UpdateData {
    expected_version: u64,
    label: String,
    geography: GeographyData,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ExpectedData {
    expected_version: u64,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct AddressData {
    address_id: UserAddressId,
    label: String,
    geography: GeographyData,
    input_revision: u64,
    version: u64,
    preferred: bool,
    resolution: AddressResolutionData,
}
impl From<UserAddressView> for AddressData {
    fn from(v: UserAddressView) -> Self {
        Self {
            address_id: v.id,
            label: v.label.as_str().into(),
            geography: v.geography.into(),
            input_revision: v.input_revision,
            version: v.version,
            preferred: v.preferred,
            resolution: v.resolution.into(),
        }
    }
}
#[derive(Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "SCREAMING_SNAKE_CASE", deny_unknown_fields)]
enum DestinationData {
    None,
    Address {
        #[serde(rename = "addressId")]
        address_id: UserAddressId,
    },
    CountryPostal {
        country: String,
        #[serde(rename = "postalCode", default)]
        postal_code: Option<String>,
    },
}
impl DestinationData {
    fn into_core(self) -> Result<BrowsingDestination, ApiError> {
        Ok(match self {
            Self::None => BrowsingDestination::None,
            Self::Address { address_id } => BrowsingDestination::Address(address_id),
            Self::CountryPostal {
                country,
                postal_code,
            } => BrowsingDestination::CountryPostal {
                country: country_from_code(&country, ReferenceRelease::CURRENT)
                    .map_err(|_| bad())?,
                postal_code: postal_code
                    .map(PostalCode::new)
                    .transpose()
                    .map_err(|_| bad())?,
            },
        })
    }
}
impl From<BrowsingDestination> for DestinationData {
    fn from(v: BrowsingDestination) -> Self {
        match v {
            BrowsingDestination::None => Self::None,
            BrowsingDestination::Address(id) => Self::Address { address_id: id },
            BrowsingDestination::CountryPostal {
                country,
                postal_code,
            } => Self::CountryPostal {
                country: country.alpha2().into(),
                postal_code: postal_code.map(|p| p.as_str().to_owned()),
            },
        }
    }
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SetDestinationData {
    expected_version: u64,
    destination: DestinationData,
}
#[derive(Serialize)]
struct DestinationResponse {
    version: u64,
    destination: DestinationData,
}
impl From<DestinationView> for DestinationResponse {
    fn from(v: DestinationView) -> Self {
        Self {
            version: v.version,
            destination: v.destination.into(),
        }
    }
}
#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub(crate) struct PageQuery {
    after: Option<UserAddressId>,
    limit: Option<u32>,
}
#[derive(Serialize)]
struct PageData {
    items: Vec<AddressData>,
    next: Option<UserAddressId>,
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
        Ok::<_, ApiError>(CreateUserAddress {
            id: d.address_id,
            label: UserAddressLabel::new(d.label).map_err(|_| bad())?,
            geography: d.geography.into_core()?,
        })
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
        Ok(v) => Json(PageData {
            items: v.items.into_iter().map(Into::into).collect(),
            next: v.next,
        })
        .into_response(),
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
        Ok::<_, ApiError>(UpdateUserAddress {
            id,
            expected_version: d.expected_version,
            label: UserAddressLabel::new(d.label).map_err(|_| bad())?,
            geography: d.geography.into_core()?,
        })
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
#[derive(Serialize)]
struct DerivedData {
    geography: Option<GeographyData>,
    position: Option<PositionData>,
}
impl From<DerivedGeography> for DerivedData {
    fn from(v: DerivedGeography) -> Self {
        Self {
            geography: v.description().cloned().map(Into::into),
            position: v.position().map(Into::into),
        }
    }
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct AddressAttributionData {
    provider: String,
    attribution_url: String,
}
#[derive(Serialize)]
struct AddressResolutionData {
    status: &'static str,
    derived: Option<DerivedData>,
    attribution: Option<AddressAttributionData>,
}
impl From<UserAddressResolution> for AddressResolutionData {
    fn from(v: UserAddressResolution) -> Self {
        Self {
            status: v.status.as_str(),
            derived: v.derived.map(Into::into),
            attribution: v.attribution.map(|a| AddressAttributionData {
                provider: a.provider,
                attribution_url: a.attribution_url,
            }),
        }
    }
}

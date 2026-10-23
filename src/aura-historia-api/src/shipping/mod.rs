mod error;
pub(crate) mod listing;
use crate::{
    auth::{TokenAuthenticator, protected_context},
    error::{
        ApiError, BAD_BODY_VALUE, FORBIDDEN, INVALID_CREDENTIALS, LISTING_SOURCE_NOT_FOUND,
        PARTY_NOT_FOUND, SHIPPING_CREATION_KEY_CONFLICT, SHIPPING_LOCATION_NOT_FOUND,
        SHIPPING_PROFILE_NOT_FOUND, SHIPPING_PROFILE_RETIRED, SHIPPING_REFERENCE_INELIGIBLE,
        SHIPPING_REVISION_EXHAUSTED, SHIPPING_STALE_REVISION, SHIPPING_STORAGE_ERROR,
        SHIPPING_TEMPORARILY_UNAVAILABLE,
    },
    wire::parse_path_object_id,
};
#[cfg(test)]
use aura_historia_contracts::shipping::terms;
use axum::{
    Json,
    extract::{Path, Query, State},
    http::HeaderMap,
    response::{IntoResponse, Response},
};
use listing_source_core::ListingSourceId;

#[cfg(test)]
use aura_historia_contracts::shipping::management::ContentData;
pub use aura_historia_contracts::shipping::management::ListQuery;
use aura_historia_contracts::shipping::management::{
    AssignData, ClearData, CreateData, DefaultData, LifecycleData, ProfileData, ReplaceData,
    SummaryData,
};
use shipping_core::profile::ShippingProfileId;
use shipping_service::{
    ShippingError,
    use_cases::{
        AssignShippingDefaultUseCase, ClearShippingDefaultUseCase, CreateShippingProfileUseCase,
        DefaultView, GetShippingDefaultUseCase, GetShippingProfileUseCase,
        ListShippingProfilesUseCase, ProfileView, ReplaceShippingProfileUseCase,
        RestoreShippingProfileUseCase, RetireShippingProfileUseCase,
    },
};
use std::sync::Arc;

#[derive(Clone)]
pub struct ShippingState {
    pub create: Arc<dyn CreateShippingProfileUseCase>,
    pub replace: Arc<dyn ReplaceShippingProfileUseCase>,
    pub retire: Arc<dyn RetireShippingProfileUseCase>,
    pub restore: Arc<dyn RestoreShippingProfileUseCase>,
    pub assign_default: Arc<dyn AssignShippingDefaultUseCase>,
    pub clear_default: Arc<dyn ClearShippingDefaultUseCase>,
    pub get: Arc<dyn GetShippingProfileUseCase>,
    pub list: Arc<dyn ListShippingProfilesUseCase>,
    pub get_default: Arc<dyn GetShippingDefaultUseCase>,
    pub authenticator: Arc<dyn TokenAuthenticator>,
}
fn bad() -> ApiError {
    ApiError::bad_request(BAD_BODY_VALUE).with_detail("Invalid advertised shipping input.")
}
fn parse<T: serde::de::DeserializeOwned>(body: &str) -> Result<T, ApiError> {
    serde_json::from_str(body).map_err(|_| bad())
}
fn profile_response(result: Result<ProfileView, ShippingError>) -> Response {
    match result.map_err(ApiError::from).and_then(|view| {
        ProfileData::try_from(view)
            .map_err(|_| ApiError::internal_server_error(SHIPPING_STORAGE_ERROR))
    }) {
        Ok(v) => Json(v).into_response(),
        Err(e) => e.into_response(),
    }
}
fn default_response(result: Result<DefaultView, ShippingError>) -> Response {
    match result {
        Ok(v) => Json(DefaultData::from(v)).into_response(),
        Err(e) => ApiError::from(e).into_response(),
    }
}
fn source_id(s: &str) -> Result<ListingSourceId, ApiError> {
    parse_path_object_id(s, "listing_source_id", "ListingSource")
}
fn profile_id(s: &str) -> Result<ShippingProfileId, ApiError> {
    parse_path_object_id(s, "shipping_profile_id", "ShippingProfile")
}

pub async fn create(
    State(s): State<ShippingState>,
    Path(source): Path<String>,
    headers: HeaderMap,
    body: String,
) -> Response {
    let (ctx, _) = match protected_context(s.authenticator.as_ref(), &headers).await {
        Ok(v) => v,
        Err(r) => return *r,
    };
    let command = (|| {
        let source_id = source_id(&source)?;
        let d: CreateData = parse(&body)?;
        d.into_command(source_id).map_err(|_| bad())
    })();
    match command {
        Ok(c) => profile_response(s.create.execute(&ctx, c).await),
        Err(e) => e.into_response(),
    }
}
pub async fn replace(
    State(s): State<ShippingState>,
    Path((source, id)): Path<(String, String)>,
    headers: HeaderMap,
    body: String,
) -> Response {
    let (ctx, _) = match protected_context(s.authenticator.as_ref(), &headers).await {
        Ok(v) => v,
        Err(r) => return *r,
    };
    let command = (|| {
        let source_id = source_id(&source)?;
        let d: ReplaceData = parse(&body)?;
        d.into_command(source_id, profile_id(&id)?)
            .map_err(|_| bad())
    })();
    match command {
        Ok(c) => profile_response(s.replace.execute(&ctx, c).await),
        Err(e) => e.into_response(),
    }
}
async fn lifecycle(
    s: ShippingState,
    source: String,
    id: String,
    headers: HeaderMap,
    body: String,
    restore: bool,
) -> Response {
    let (ctx, _) = match protected_context(s.authenticator.as_ref(), &headers).await {
        Ok(v) => v,
        Err(r) => return *r,
    };
    let command = (|| {
        let d: LifecycleData = parse(&body)?;
        d.into_command(source_id(&source)?, profile_id(&id)?)
            .map_err(|_| bad())
    })();
    match command {
        Ok(c) => profile_response(if restore {
            s.restore.execute(&ctx, c).await
        } else {
            s.retire.execute(&ctx, c).await
        }),
        Err(e) => e.into_response(),
    }
}
pub async fn retire(
    State(s): State<ShippingState>,
    Path((source, id)): Path<(String, String)>,
    headers: HeaderMap,
    body: String,
) -> Response {
    lifecycle(s, source, id, headers, body, false).await
}
pub async fn restore(
    State(s): State<ShippingState>,
    Path((source, id)): Path<(String, String)>,
    headers: HeaderMap,
    body: String,
) -> Response {
    lifecycle(s, source, id, headers, body, true).await
}
pub async fn get(
    State(s): State<ShippingState>,
    Path((source, id)): Path<(String, String)>,
    headers: HeaderMap,
) -> Response {
    let (ctx, _) = match protected_context(s.authenticator.as_ref(), &headers).await {
        Ok(v) => v,
        Err(r) => return *r,
    };
    match source_id(&source).and_then(|source| Ok((source, profile_id(&id)?))) {
        Ok((source, id)) => profile_response(s.get.execute(&ctx, source, id).await),
        Err(e) => e.into_response(),
    }
}
pub async fn list(
    State(s): State<ShippingState>,
    Path(source): Path<String>,
    headers: HeaderMap,
    query: Result<Query<ListQuery>, axum::extract::rejection::QueryRejection>,
) -> Response {
    let (ctx, _) = match protected_context(s.authenticator.as_ref(), &headers).await {
        Ok(v) => v,
        Err(r) => return *r,
    };
    let request = source_id(&source).and_then(|source| {
        let Query(q) = query.map_err(|_| bad())?;
        let after = q.after;
        let size = q.size;
        let identity = q.into_identity().map_err(|_| bad())?;
        Ok((source, after, size, identity))
    });
    match request {
        Ok((source, after, size, identity)) => match s
            .list
            .execute(&ctx, source, after, size.unwrap_or(21), identity)
            .await
        {
            Ok(v) => Json(v.into_iter().map(SummaryData::from).collect::<Vec<_>>()).into_response(),
            Err(e) => ApiError::from(e).into_response(),
        },
        Err(e) => e.into_response(),
    }
}
pub async fn get_default(
    State(s): State<ShippingState>,
    Path(source): Path<String>,
    headers: HeaderMap,
) -> Response {
    let (ctx, _) = match protected_context(s.authenticator.as_ref(), &headers).await {
        Ok(v) => v,
        Err(r) => return *r,
    };
    match source_id(&source) {
        Ok(source) => default_response(s.get_default.execute(&ctx, source).await),
        Err(e) => e.into_response(),
    }
}
pub async fn assign_default(
    State(s): State<ShippingState>,
    Path(source): Path<String>,
    headers: HeaderMap,
    body: String,
) -> Response {
    let (ctx, _) = match protected_context(s.authenticator.as_ref(), &headers).await {
        Ok(v) => v,
        Err(r) => return *r,
    };
    let command = (|| {
        let d: AssignData = parse(&body)?;
        d.into_command(source_id(&source)?).map_err(|_| bad())
    })();
    match command {
        Ok(c) => default_response(s.assign_default.execute(&ctx, c).await),
        Err(e) => e.into_response(),
    }
}
pub async fn clear_default(
    State(s): State<ShippingState>,
    Path(source): Path<String>,
    headers: HeaderMap,
    body: String,
) -> Response {
    let (ctx, _) = match protected_context(s.authenticator.as_ref(), &headers).await {
        Ok(v) => v,
        Err(r) => return *r,
    };
    let command = (|| {
        let d: ClearData = parse(&body)?;
        d.into_command(source_id(&source)?).map_err(|_| bad())
    })();
    match command {
        Ok(c) => default_response(s.clear_default.execute(&ctx, c).await),
        Err(e) => e.into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use shipping_core::{inputs::Applicability, terms::*, time::CollectionArrangement};
    #[test]
    fn public_terms_ids_roundtrip_without_accepting_storage_uuid_text() {
        let source = ListingSourceId::new();
        let location = party_core::party_location_id::PartyLocationId::new();
        let terms = DeliveryTerms::new(
            Some(ShippingPolicy::Prohibited),
            Some(CollectionPolicy::new(
                Applicability::default(),
                CollectionDependency::Independent,
                Some(ShippingEligibility::Offered),
                Some(DeliverySite::PartyLocation(location)),
                Some(CollectionArrangement::ByAppointment),
            )),
        );
        let mut value = terms::encode(&terms).unwrap();
        assert_eq!(
            location.to_string(),
            value["collection"]["site"]["party_location_id"]
        );
        assert_eq!(terms, terms::decode(value.clone(), source).unwrap());
        value["collection"]["site"]["party_location_id"] = json!(location.into_uuid().to_string());
        assert!(terms::decode(value, source).is_err());
    }
    #[test]
    fn revisions_and_private_causes_are_safe_at_api_boundary() {
        assert!(parse::<LifecycleData>(r#"{"expectedRevision":0}"#).is_err());
        assert!(parse::<LifecycleData>(r#"{"expectedRevision":-1}"#).is_err());
        assert!(parse::<ContentData>(r#"{"name":"Advertised terms","terms":{"schema":"SHIP_TERMS_V1","shipping":null,"collection":null}}"#).is_err());
        assert!(parse::<ContentData>(r#"{"name":"Advertised terms","attributedPartyId":null,"terms":{"schema":"SHIP_TERMS_V1","shipping":null,"collection":null}}"#).is_ok());
        assert!(terms::decode(json!({"schema":"SHIP_TERMS_V1","shipping":{"kind":"PROHIBITED","extra":"private"},"collection":null}),ListingSourceId::new()).is_err());
        let error = ApiError::from(ShippingError::CorruptShippingState {
            source: Box::new(std::io::Error::other("private evidence")),
        });
        assert!(
            !serde_json::to_string(&error)
                .unwrap()
                .contains("private evidence")
        );
    }
}

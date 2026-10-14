mod terms;
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
use axum::{
    Json,
    extract::{Path, Query, State},
    http::HeaderMap,
    response::{IntoResponse, Response},
};
use listing_source_core::ListingSourceId;
use party_core::party_id::PartyId;
use serde::{Deserialize, Serialize};
use shipping_core::profile::{ShippingProfileContent, ShippingProfileId, ShippingProfileName};
use shipping_service::{
    ShippingError,
    ports::{ShippingDefaultRevision, ShippingEvidence, ShippingManagementRevision},
    use_cases::{
        AssignDefaultCommand, AssignShippingDefaultUseCase, ClearDefaultCommand,
        ClearShippingDefaultUseCase, CreateProfileCommand, CreateShippingProfileUseCase,
        DefaultEligibility, DefaultView, GetShippingDefaultUseCase, GetShippingProfileUseCase,
        ListShippingProfilesUseCase, ProfileLifecycleCommand, ProfileSummary, ProfileView,
        ReplaceProfileCommand, ReplaceShippingProfileUseCase, RestoreShippingProfileUseCase,
        RetireShippingProfileUseCase,
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
struct RequiredNullable<T>(Option<T>);
fn required<'de, D: serde::Deserializer<'de>, T: Deserialize<'de>>(
    d: D,
) -> Result<RequiredNullable<T>, D::Error> {
    Option::deserialize(d).map(RequiredNullable)
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ContentData {
    name: String,
    #[serde(deserialize_with = "required")]
    attributed_party_id: RequiredNullable<PartyId>,
    terms: serde_json::Value,
}
impl ContentData {
    fn domain(self, source: ListingSourceId) -> Result<ShippingProfileContent, ApiError> {
        Ok(ShippingProfileContent {
            name: ShippingProfileName::new(self.name).map_err(|_| bad())?,
            attributed_party: self.attributed_party_id.0,
            terms: terms::decode(self.terms, source).map_err(|_| bad())?,
        })
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct EvidenceData {
    reference: String,
    assertion: String,
}
fn evidence(data: Vec<EvidenceData>) -> Result<Vec<ShippingEvidence>, ApiError> {
    data.into_iter()
        .map(|e| ShippingEvidence::new(e.reference, e.assertion).map_err(|_| bad()))
        .collect()
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CreateData {
    creation_key: String,
    content: ContentData,
    evidence: Vec<EvidenceData>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ReplaceData {
    expected_revision: ShippingManagementRevision,
    content: ContentData,
    evidence: Vec<EvidenceData>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct LifecycleData {
    expected_revision: ShippingManagementRevision,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct AssignData {
    expected_revision: ShippingDefaultRevision,
    shipping_profile_id: ShippingProfileId,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ClearData {
    expected_revision: ShippingDefaultRevision,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ListQuery {
    after: Option<ShippingProfileId>,
    size: Option<u32>,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ProfileData {
    shipping_profile_id: ShippingProfileId,
    listing_source_id: ListingSourceId,
    management_revision: ShippingManagementRevision,
    terms_revision: u64,
    lifecycle: &'static str,
    content: ProfileContentData,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ProfileContentData {
    name: String,
    attributed_party_id: Option<PartyId>,
    terms: serde_json::Value,
}
impl TryFrom<ProfileView> for ProfileData {
    type Error = ApiError;
    fn try_from(v: ProfileView) -> Result<Self, Self::Error> {
        Ok(Self {
            shipping_profile_id: v.id,
            listing_source_id: v.source_id,
            management_revision: v.management_revision,
            terms_revision: v.revision.into_inner(),
            lifecycle: v.lifecycle.as_str(),
            content: ProfileContentData {
                name: v.content.name.as_str().into(),
                attributed_party_id: v.content.attributed_party,
                terms: terms::encode(&v.content.terms)
                    .map_err(|_| ApiError::internal_server_error(SHIPPING_STORAGE_ERROR))?,
            },
        })
    }
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SummaryData {
    shipping_profile_id: ShippingProfileId,
    listing_source_id: ListingSourceId,
    name: String,
    management_revision: ShippingManagementRevision,
    terms_revision: u64,
    lifecycle: &'static str,
}
impl From<ProfileSummary> for SummaryData {
    fn from(v: ProfileSummary) -> Self {
        Self {
            shipping_profile_id: v.id,
            listing_source_id: v.source_id,
            name: v.name.as_str().into(),
            management_revision: v.management_revision,
            terms_revision: v.revision.into_inner(),
            lifecycle: v.lifecycle.as_str(),
        }
    }
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct DefaultData {
    listing_source_id: ListingSourceId,
    revision: ShippingDefaultRevision,
    shipping_profile_id: Option<ShippingProfileId>,
    eligibility: Option<&'static str>,
}
impl From<DefaultView> for DefaultData {
    fn from(v: DefaultView) -> Self {
        Self {
            listing_source_id: v.source_id,
            revision: v.revision,
            shipping_profile_id: v.profile_id,
            eligibility: v.eligibility.map(|e| match e {
                DefaultEligibility::Active => "ACTIVE",
                DefaultEligibility::Retired => "RETIRED",
                DefaultEligibility::Missing => "MISSING",
            }),
        }
    }
}
impl From<ShippingError> for ApiError {
    fn from(e: ShippingError) -> Self {
        match e {
            ShippingError::AuthenticationRequired => Self::unauthorized(INVALID_CREDENTIALS),
            ShippingError::SourceAccessDenied => Self::forbidden(FORBIDDEN),
            ShippingError::SourceNotFound => Self::not_found(LISTING_SOURCE_NOT_FOUND),
            ShippingError::ProfileNotFound => Self::not_found(SHIPPING_PROFILE_NOT_FOUND),
            ShippingError::AttributedPartyNotFound { .. } => Self::not_found(PARTY_NOT_FOUND),
            ShippingError::LocationNotFound => Self::not_found(SHIPPING_LOCATION_NOT_FOUND),
            ShippingError::InvalidInput
            | ShippingError::InvalidRevision
            | ShippingError::InvalidTerms(shipping_core::InvalidShippingValue::SourceMismatch) => {
                bad()
            }
            ShippingError::InvalidTerms(shipping_core::InvalidShippingValue::RetiredProfile)
            | ShippingError::LocationRetired => Self::conflict(SHIPPING_REFERENCE_INELIGIBLE),
            ShippingError::InvalidTerms(_)
            | ShippingError::Profile(shipping_core::profile::ShippingProfileError::Invalid(_)) => {
                bad()
            }
            ShippingError::StaleRevision => Self::conflict(SHIPPING_STALE_REVISION),
            ShippingError::CreationKeyConflict => Self::conflict(SHIPPING_CREATION_KEY_CONFLICT),
            ShippingError::RevisionExhausted
            | ShippingError::Profile(
                shipping_core::profile::ShippingProfileError::RevisionExhausted,
            ) => Self::conflict(SHIPPING_REVISION_EXHAUSTED),
            ShippingError::Profile(shipping_core::profile::ShippingProfileError::Retired) => {
                Self::conflict(SHIPPING_PROFILE_RETIRED)
            }
            ShippingError::PersistenceUnavailable { .. } | ShippingError::Transaction(_) => {
                Self::service_unavailable(SHIPPING_TEMPORARILY_UNAVAILABLE)
            }
            ShippingError::CorruptShippingState { .. }
            | ShippingError::PersistenceFailed { .. } => {
                Self::internal_server_error(SHIPPING_STORAGE_ERROR)
            }
        }
    }
}
fn profile_response(result: Result<ProfileView, ShippingError>) -> Response {
    match result
        .map_err(ApiError::from)
        .and_then(ProfileData::try_from)
    {
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
        Ok::<_, ApiError>(CreateProfileCommand {
            source_id,
            creation_key: d.creation_key,
            content: d.content.domain(source_id)?,
            evidence: evidence(d.evidence)?,
        })
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
        Ok::<_, ApiError>(ReplaceProfileCommand {
            source_id,
            profile_id: profile_id(&id)?,
            expected_revision: d.expected_revision,
            content: d.content.domain(source_id)?,
            evidence: evidence(d.evidence)?,
        })
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
        Ok::<_, ApiError>(ProfileLifecycleCommand {
            source_id: source_id(&source)?,
            profile_id: profile_id(&id)?,
            expected_revision: d.expected_revision,
        })
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
        Ok((source, q))
    });
    match request {
        Ok((source, q)) => match s
            .list
            .execute(&ctx, source, q.after, q.size.unwrap_or(21))
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
        Ok::<_, ApiError>(AssignDefaultCommand {
            source_id: source_id(&source)?,
            profile_id: d.shipping_profile_id,
            expected_revision: d.expected_revision,
        })
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
        Ok::<_, ApiError>(ClearDefaultCommand {
            source_id: source_id(&source)?,
            expected_revision: d.expected_revision,
        })
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

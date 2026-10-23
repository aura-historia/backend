//! Granted-source discovery has its own complete, private page contract.
use crate::product_listing_management::QueryInputError;
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use listing_source_core::{ListingSourceId, ListingSourceName};
use partnership_service::{
    ports::{AdministeredListingSource, AdministeredListingSourcePosition},
    use_cases::queries::list_administered_listing_sources::{
        AdministeredListingSourcesContinuation, ListAdministeredListingSourcesRequest,
        ListAdministeredListingSourcesResult,
    },
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use user_core::user_id::UserId;
const ORDER: &str = "name-c-asc-id-asc";

#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AdministeredListingSourcesQuery {
    /// Defaults to 50. Allowed range is 1 through 100.
    #[schemars(range(min = 1, max = 100))]
    pub size: Option<u32>,
    /// Opaque continuation bound to the authenticated actor and source ordering.
    #[schemars(length(max = 4096))]
    pub after: Option<String>,
}
impl AdministeredListingSourcesQuery {
    pub fn into_request(self) -> Result<ListAdministeredListingSourcesRequest, QueryInputError> {
        let size = self.size.unwrap_or(50);
        if !(1..=100).contains(&size) {
            return Err(QueryInputError::invalid(
                "size",
                "size must be between 1 and 100.",
            ));
        }
        Ok(ListAdministeredListingSourcesRequest {
            size,
            after: self.after.as_deref().map(decode).transpose()?,
        })
    }
}
#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct AdministeredListingSourceData {
    #[schemars(schema_with = "crate::wire::schema::listing_source_id")]
    listing_source_id: ListingSourceId,
    listing_source_slug_id: String,
    name: String,
}
impl From<AdministeredListingSource> for AdministeredListingSourceData {
    fn from(value: AdministeredListingSource) -> Self {
        Self {
            listing_source_id: value.listing_source_id,
            listing_source_slug_id: value.slug_id.to_string(),
            name: value.name.to_string(),
        }
    }
}
#[derive(Debug, Serialize, JsonSchema)]
pub struct AdministeredListingSourcesPageData {
    pub items: Vec<AdministeredListingSourceData>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next: Option<String>,
}
impl From<ListAdministeredListingSourcesResult> for AdministeredListingSourcesPageData {
    fn from(value: ListAdministeredListingSourcesResult) -> Self {
        Self {
            items: value.items.into_iter().map(Into::into).collect(),
            next: value.next.map(encode),
        }
    }
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CursorData {
    version: u8,
    order: String,
    user_id: UserId,
    name: String,
    listing_source_id: ListingSourceId,
}
fn invalid_cursor() -> QueryInputError {
    QueryInputError::invalid("after", "Invalid administered-source continuation.")
}
fn decode(value: &str) -> Result<AdministeredListingSourcesContinuation, QueryInputError> {
    if value.len() > 4096 {
        return Err(invalid_cursor());
    }
    let bytes = URL_SAFE_NO_PAD
        .decode(value)
        .map_err(|_| invalid_cursor())?;
    let data: CursorData = serde_json::from_slice(&bytes).map_err(|_| invalid_cursor())?;
    if data.version != 1 || data.order != ORDER || data.name.contains('\0') {
        return Err(invalid_cursor());
    }
    let name = ListingSourceName::try_from(data.name.as_str()).map_err(|_| invalid_cursor())?;
    if name.as_ref() != data.name {
        return Err(invalid_cursor());
    }
    Ok(AdministeredListingSourcesContinuation {
        user_id: data.user_id,
        position: AdministeredListingSourcePosition {
            name,
            listing_source_id: data.listing_source_id,
        },
    })
}
fn encode(value: AdministeredListingSourcesContinuation) -> String {
    URL_SAFE_NO_PAD.encode(
        serde_json::to_vec(&CursorData {
            version: 1,
            order: ORDER.into(),
            user_id: value.user_id,
            name: value.position.name.to_string(),
            listing_source_id: value.position.listing_source_id,
        })
        .expect("typed administered-source continuation"),
    )
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn should_preserve_actor_and_position_and_reject_invalid_continuations() {
        let continuation = AdministeredListingSourcesContinuation {
            user_id: UserId::new(),
            position: AdministeredListingSourcePosition {
                name: ListingSourceName::try_from("Antiques").unwrap(),
                listing_source_id: ListingSourceId::new(),
            },
        };
        let encoded = encode(continuation.clone());
        assert_eq!(continuation, decode(&encoded).unwrap());
        let mut data: serde_json::Value =
            serde_json::from_slice(&URL_SAFE_NO_PAD.decode(encoded).unwrap()).unwrap();
        for (field, invalid) in [
            ("version", serde_json::json!(2)),
            ("order", serde_json::json!("id-asc")),
            ("userId", serde_json::json!("invalid")),
            ("listingSourceId", serde_json::json!("invalid")),
            ("name", serde_json::json!(" ")),
            ("name", serde_json::json!("bad\0name")),
        ] {
            let original = data[field].clone();
            data[field] = invalid;
            assert!(decode(&URL_SAFE_NO_PAD.encode(serde_json::to_vec(&data).unwrap())).is_err());
            data[field] = original;
        }
        assert!(decode("garbage").is_err());
        assert!(decode(&"a".repeat(4097)).is_err());
    }
}

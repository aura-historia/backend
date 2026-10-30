//! Opaque `searchAfter` codec for keyset-paginated admin searches.
//!
//! A cursor carries the scope binding, the stored sort key and the ID of the last returned item.
//! Clients pass it back unchanged with the same filters and sort; its content is not part of the
//! API contract.

use crate::error::{ApiError, BAD_QUERY_PARAMETER_VALUE};
use application::pagination::{KeysetCursor, ScopeBinding};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use serde::de::{DeserializeOwned, Error as _};
use serde::{Deserialize, Deserializer, Serialize};
use std::fmt::Display;
use std::str::FromStr;

const FIELD: &str = "searchAfter";
const VERSION: u8 = 1;
const MAX_ENCODED_BYTES: usize = 4 * 1024;

/// Text sort value. PostgreSQL `text` cannot hold NUL, so decoding rejects it as client input
/// instead of letting the query fail.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub(crate) struct CursorText(#[serde(deserialize_with = "nul_free")] pub(crate) String);

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CursorData<K> {
    version: u8,
    scope: String,
    key: K,
    id: String,
}

pub(crate) fn encode<K: Serialize, I: Display>(cursor: KeysetCursor<K, I>) -> String {
    let data = CursorData {
        version: VERSION,
        scope: URL_SAFE_NO_PAD.encode(cursor.scope.as_bytes()),
        key: cursor.sort_key,
        id: cursor.id.to_string(),
    };
    URL_SAFE_NO_PAD.encode(serde_json::to_vec(&data).expect("keyset cursor serializes"))
}

pub(crate) fn decode<K: DeserializeOwned, I: FromStr>(
    value: &str,
) -> Result<KeysetCursor<K, I>, ApiError> {
    if value.len() > MAX_ENCODED_BYTES {
        return Err(invalid());
    }
    let bytes = URL_SAFE_NO_PAD.decode(value).map_err(|_| invalid())?;
    let data = serde_json::from_slice::<CursorData<K>>(&bytes).map_err(|_| invalid())?;
    if data.version != VERSION {
        return Err(invalid());
    }
    let scope = URL_SAFE_NO_PAD
        .decode(data.scope)
        .ok()
        .and_then(|bytes| <[u8; 32]>::try_from(bytes).ok())
        .map(ScopeBinding::from_bytes)
        .ok_or_else(invalid)?;
    Ok(KeysetCursor {
        scope,
        sort_key: data.key,
        id: data.id.parse().map_err(|_| invalid())?,
    })
}

/// A well-formed cursor presented with filters or a sort other than those that returned it.
pub(crate) fn foreign_scope() -> ApiError {
    ApiError::bad_request(BAD_QUERY_PARAMETER_VALUE)
        .with_query_field(FIELD)
        .with_detail(
            "searchAfter belongs to different filters or sort. Repeat the request that returned it, or start again without searchAfter.",
        )
}

fn invalid() -> ApiError {
    ApiError::bad_request(BAD_QUERY_PARAMETER_VALUE)
        .with_query_field(FIELD)
        .with_detail("searchAfter must be a cursor returned by this search.")
}

fn nul_free<'de, D: Deserializer<'de>>(deserializer: D) -> Result<String, D::Error> {
    let value = String::deserialize(deserializer)?;
    if value.contains('\0') {
        return Err(D::Error::custom("cursor text contains NUL"));
    }
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use party_core::party_id::PartyId;
    use serde_json::{Value, json};
    use time::OffsetDateTime;

    #[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
    #[serde(rename_all = "camelCase")]
    enum Key {
        Name(Option<CursorText>),
        Created(#[serde(with = "time::serde::timestamp::nanoseconds")] OffsetDateTime),
    }

    fn decode_party(value: &str) -> Result<KeysetCursor<Key, PartyId>, ApiError> {
        decode(value)
    }

    /// A well-formed Party cursor payload with `field` replaced.
    fn payload_with(field: &str, value: Value) -> String {
        let mut payload = json!({
            "version": VERSION,
            "scope": URL_SAFE_NO_PAD.encode([7; 32]),
            "key": {"name": null},
            "id": PartyId::new().to_string(),
        });
        payload[field] = value;
        URL_SAFE_NO_PAD.encode(payload.to_string())
    }

    #[test]
    fn should_round_trip_scope_key_and_id() {
        for key in [
            Key::Name(Some(CursorText("Zoë \"quoted\" \\ name".to_owned()))),
            Key::Name(None),
            Key::Created(OffsetDateTime::UNIX_EPOCH + time::Duration::nanoseconds(123_456_789)),
        ] {
            let scope = ScopeBinding::builder("test/v1").finish();
            let id = PartyId::new();
            let encoded = encode(KeysetCursor {
                scope,
                sort_key: &key,
                id,
            });

            assert_eq!(
                KeysetCursor {
                    scope,
                    sort_key: key,
                    id,
                },
                decode_party(&encoded)
                    .unwrap_or_else(|error| panic!("cursor must round trip: {error:?}"))
            );
        }
    }

    #[test]
    fn should_reject_cursors_that_this_search_did_not_issue() {
        for value in [
            PartyId::new().to_string(),
            "not base64!".to_owned(),
            URL_SAFE_NO_PAD.encode(json!(["name", PartyId::new().to_string()]).to_string()),
            payload_with("version", json!(2)),
            payload_with("scope", json!(URL_SAFE_NO_PAD.encode([7; 31]))),
            payload_with("key", json!({"name": "a\u{0}b"})),
            payload_with("key", json!({"other": "a"})),
            payload_with(
                "id",
                json!(listing_source_core::ListingSourceId::new().to_string()),
            ),
            payload_with("extra", json!(1)),
            "A".repeat(MAX_ENCODED_BYTES + 1),
        ] {
            let error = decode_party(&value).expect_err("foreign cursor must be rejected");
            assert_eq!(BAD_QUERY_PARAMETER_VALUE, error.code(), "{value}");
        }
    }
}

use crate::source_listing_id::SourceListingId;
use sha2::{Digest, Sha256};
use std::fmt;

/// Maximum UTF-8 byte length of the source-scoped public route locator.
pub const MAX_SOURCE_LISTING_SLUG_ID_BYTES: usize = 255;
const HASH_SUFFIX_HEX_LENGTH: usize = 20;
const BODY_MAX_BYTES: usize = MAX_SOURCE_LISTING_SLUG_ID_BYTES - 1 - HASH_SUFFIX_HEX_LENGTH;

/// Immutable, source-scoped public route locator derived from a canonical
/// `SourceListingId`. The 80-bit SHA-256 suffix preserves identity when raw
/// source IDs normalize to the same readable body.
#[derive(
    Debug, Clone, PartialEq, PartialOrd, Eq, Ord, Hash, serde::Serialize, serde::Deserialize,
)]
#[serde(into = "String", try_from = "String")]
pub struct SourceListingSlugId(String);

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("invalid source listing slug ID")]
pub struct InvalidSourceListingSlugId;

impl SourceListingSlugId {
    pub fn from_source_listing_id(source_listing_id: &SourceListingId) -> Self {
        let mut body = slug_body(source_listing_id.as_ref());
        body.truncate(BODY_MAX_BYTES);
        let body = body.trim_end_matches('-');
        let body = if body.is_empty() { "listing" } else { body };
        let digest = Sha256::digest(source_listing_id.as_ref().as_bytes());
        let suffix = digest
            .iter()
            .take(HASH_SUFFIX_HEX_LENGTH / 2)
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        Self(format!("{body}-{suffix}"))
    }

    pub fn raw(value: &str) -> Result<Self, InvalidSourceListingSlugId> {
        let Some((body, suffix)) = value.rsplit_once('-') else {
            return Err(InvalidSourceListingSlugId);
        };
        if value.len() > MAX_SOURCE_LISTING_SLUG_ID_BYTES
            || body.is_empty()
            || suffix.len() != HASH_SUFFIX_HEX_LENGTH
            || !suffix.bytes().all(|byte| {
                byte.is_ascii_digit() || (byte.is_ascii_lowercase() && byte.is_ascii_hexdigit())
            })
            || !body
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
            || body.starts_with('-')
            || body.ends_with('-')
            || body.contains("--")
        {
            return Err(InvalidSourceListingSlugId);
        }
        Ok(Self(value.to_owned()))
    }
}

impl TryFrom<String> for SourceListingSlugId {
    type Error = InvalidSourceListingSlugId;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::raw(&value)
    }
}

impl AsRef<str> for SourceListingSlugId {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for SourceListingSlugId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_ref())
    }
}

impl From<SourceListingSlugId> for String {
    fn from(value: SourceListingSlugId) -> Self {
        value.0
    }
}

fn slug_body(value: &str) -> String {
    let mut body = String::new();
    let mut previous_was_separator = false;
    for character in value.chars() {
        if character.is_ascii_alphanumeric() {
            body.push(character.to_ascii_lowercase());
            previous_was_separator = false;
        } else if !previous_was_separator && !body.is_empty() {
            body.push('-');
            previous_was_separator = true;
        }
    }
    let body = body.trim_end_matches('-');
    if body.is_empty() {
        "listing".to_owned()
    } else {
        body.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::{
        HASH_SUFFIX_HEX_LENGTH, InvalidSourceListingSlugId, MAX_SOURCE_LISTING_SLUG_ID_BYTES,
        SourceListingSlugId,
    };
    use crate::source_listing_id::SourceListingId;

    fn raw(value: &str) -> SourceListingId {
        SourceListingId::try_from(value)
            .unwrap_or_else(|error| panic!("valid source listing ID: {error}"))
    }

    #[test]
    fn should_derive_stable_80_bit_sha256_suffix_from_raw_source_listing_id() {
        let slug = SourceListingSlugId::from_source_listing_id(&raw("SKU  #42/Blue"));
        assert_eq!("sku-42-blue-f4b0c19f13dd107fa784", slug.as_ref());
        assert_eq!(
            HASH_SUFFIX_HEX_LENGTH,
            slug.as_ref()
                .rsplit_once('-')
                .map_or(0, |(_, suffix)| suffix.len())
        );
    }

    #[test]
    fn should_keep_distinct_raw_ids_distinct_when_slug_bodies_match() {
        assert_ne!(
            SourceListingSlugId::from_source_listing_id(&raw("SKU/42")),
            SourceListingSlugId::from_source_listing_id(&raw("SKU 42"))
        );
    }

    #[test]
    fn should_hash_full_raw_id_after_truncating_readable_body() {
        let prefix = "a".repeat(300);
        assert_ne!(
            SourceListingSlugId::from_source_listing_id(&raw(&format!("{prefix}1"))),
            SourceListingSlugId::from_source_listing_id(&raw(&format!("{prefix}2")))
        );
    }

    #[test]
    fn should_cap_long_locator_and_use_fallback_for_non_ascii_body() {
        assert!(
            SourceListingSlugId::from_source_listing_id(&raw(&"a".repeat(512)))
                .as_ref()
                .len()
                <= MAX_SOURCE_LISTING_SLUG_ID_BYTES
        );
        assert!(
            SourceListingSlugId::from_source_listing_id(&raw("世界"))
                .as_ref()
                .starts_with("listing-")
        );
    }

    #[test]
    fn should_reject_invalid_raw_and_serde_values() {
        for value in [
            "listing-ABCDEF0123456789abcd",
            "listing-abcdef",
            "listing--abcdef0123456789abcd",
            &format!(
                "{}-abcdef0123456789abcd",
                "a".repeat(MAX_SOURCE_LISTING_SLUG_ID_BYTES)
            ),
        ] {
            assert_eq!(
                SourceListingSlugId::raw(value),
                Err(InvalidSourceListingSlugId)
            );
            assert!(serde_json::from_str::<SourceListingSlugId>(&format!("\"{value}\"")).is_err());
        }
    }

    #[test]
    fn should_round_trip_valid_serde_value() {
        let value = SourceListingSlugId::from_source_listing_id(&raw("sku-42"));
        let json =
            serde_json::to_string(&value).unwrap_or_else(|error| panic!("serialize: {error}"));
        let parsed = serde_json::from_str::<SourceListingSlugId>(&json)
            .unwrap_or_else(|error| panic!("deserialize: {error}"));
        assert_eq!(parsed, value);
    }
}

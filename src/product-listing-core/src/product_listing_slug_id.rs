use std::fmt;

/// Maximum UTF-8 byte length of a public product-listing title slug.
pub const MAX_PRODUCT_LISTING_SLUG_ID_BYTES: usize = 120;
const HASH_SUFFIX_HEX_LENGTH: usize = 6;
const BODY_MAX_BYTES: usize = MAX_PRODUCT_LISTING_SLUG_ID_BYTES - 1 - HASH_SUFFIX_HEX_LENGTH;

/// Immutable public route locator derived from a listing title.
#[derive(
    Debug, Clone, PartialEq, PartialOrd, Eq, Ord, Hash, serde::Serialize, serde::Deserialize,
)]
#[serde(into = "String", try_from = "String")]
pub struct ProductListingSlugId(String);

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("invalid product listing slug ID")]
pub struct InvalidProductListingSlugId;

impl ProductListingSlugId {
    /// Builds a canonical slug from a title and application-selected suffix.
    ///
    /// The core owns title normalization and validation; callers own entropy.
    pub fn from_title_and_suffix(
        title: &str,
        suffix: &str,
    ) -> Result<Self, InvalidProductListingSlugId> {
        if suffix.len() != HASH_SUFFIX_HEX_LENGTH
            || !suffix
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
        {
            return Err(InvalidProductListingSlugId);
        }

        Self::raw(&Self::canonical_title_slug(title, suffix))
    }

    fn canonical_title_slug(title: &str, suffix: &str) -> String {
        let mut body = slug::slugify(title);
        body.truncate(BODY_MAX_BYTES);
        let body = body.trim_end_matches('-');
        let body = if body.is_empty() { "listing" } else { body };
        format!("{body}-{suffix}")
    }

    pub fn raw(value: &str) -> Result<Self, InvalidProductListingSlugId> {
        let Some((body, suffix)) = value.rsplit_once('-') else {
            return Err(InvalidProductListingSlugId);
        };
        if value.len() > MAX_PRODUCT_LISTING_SLUG_ID_BYTES
            || body.is_empty()
            || suffix.len() != HASH_SUFFIX_HEX_LENGTH
            || !suffix
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
            || !body
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
            || body.starts_with('-')
            || body.ends_with('-')
            || body.contains("--")
        {
            return Err(InvalidProductListingSlugId);
        }
        Ok(Self(value.to_owned()))
    }
}

/// Deterministic compatibility conversion for title fixture inputs.
///
/// Production callers must use `raw` for an existing identifier or let the
/// application service select a random candidate with `from_title_and_suffix`.
impl From<&str> for ProductListingSlugId {
    fn from(title: &str) -> Self {
        // The fixed valid suffix keeps this conversion deterministic. The
        // service creation path never uses it, because global collision retry
        // requires a fresh candidate per attempt.
        Self(Self::canonical_title_slug(title, "000000"))
    }
}

impl TryFrom<String> for ProductListingSlugId {
    type Error = InvalidProductListingSlugId;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::raw(&value)
    }
}

impl AsRef<str> for ProductListingSlugId {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ProductListingSlugId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_ref())
    }
}

impl From<ProductListingSlugId> for String {
    fn from(value: ProductListingSlugId) -> Self {
        value.0
    }
}

#[cfg(test)]
mod tests {
    use super::{
        InvalidProductListingSlugId, MAX_PRODUCT_LISTING_SLUG_ID_BYTES, ProductListingSlugId,
    };

    #[test]
    fn should_derive_slug_from_title_with_supplied_lower_hex_suffix() {
        let slug =
            ProductListingSlugId::from_title_and_suffix("Museum Cabinet 18./19. Century", "a1b2c3")
                .unwrap_or_else(|error| panic!("valid slug: {error}"));
        let (body, suffix) = slug
            .as_ref()
            .rsplit_once('-')
            .unwrap_or_else(|| panic!("slug has suffix"));

        assert_eq!(body, "museum-cabinet-18-19-century");
        assert_eq!(suffix.len(), 6);
        assert!(
            suffix
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
        );
    }

    #[test]
    fn should_build_same_slug_for_same_title_and_suffix() {
        let first = ProductListingSlugId::from_title_and_suffix("Museum Cabinet", "a1b2c3")
            .unwrap_or_else(|error| panic!("valid slug: {error}"));
        let second = ProductListingSlugId::from_title_and_suffix("Museum Cabinet", "a1b2c3")
            .unwrap_or_else(|error| panic!("valid slug: {error}"));

        assert_eq!(first, second);
    }

    #[test]
    fn should_cap_slug_and_fall_back_to_listing_body() {
        let long_slug = ProductListingSlugId::from_title_and_suffix(&"a".repeat(200), "a1b2c3")
            .unwrap_or_else(|error| panic!("valid slug: {error}"));
        assert_eq!(long_slug.as_ref().len(), MAX_PRODUCT_LISTING_SLUG_ID_BYTES);

        let fallback_slug = ProductListingSlugId::from_title_and_suffix("", "a1b2c3")
            .unwrap_or_else(|error| panic!("valid slug: {error}"));
        assert_eq!(fallback_slug.as_ref(), "listing-a1b2c3");
    }

    #[test]
    fn should_reject_invalid_raw_and_serde_values() {
        for value in [
            "listing-ABCDEF",
            "listing-abcde",
            "listing--abcdef",
            &format!("{}-abcdef", "a".repeat(MAX_PRODUCT_LISTING_SLUG_ID_BYTES)),
        ] {
            assert_eq!(
                ProductListingSlugId::raw(value),
                Err(InvalidProductListingSlugId)
            );
            assert!(serde_json::from_str::<ProductListingSlugId>(&format!("\"{value}\"")).is_err());
        }
    }

    #[test]
    fn should_round_trip_valid_raw_and_serde_value() {
        let value = ProductListingSlugId::raw("antique-vase-a1b2c3")
            .unwrap_or_else(|error| panic!("valid raw slug: {error}"));
        let json =
            serde_json::to_string(&value).unwrap_or_else(|error| panic!("serialize: {error}"));
        let parsed = serde_json::from_str::<ProductListingSlugId>(&json)
            .unwrap_or_else(|error| panic!("deserialize: {error}"));

        assert_eq!(parsed, value);
    }
}

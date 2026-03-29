use prefixed_api_key::{
    PrefixedApiKey, PrefixedApiKeyController,
    sha2::{Digest, Sha256},
};
use serde::{Deserialize, Serialize};

const PARTNER_SHOP_API_KEY_PREFIX: &str = "aurahistoria";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(into = "String", try_from = "String")]
pub struct PartnerShopApiKey(String);

#[derive(Debug, Clone, thiserror::Error)]
#[error("PartnerShopApiKey '{0}' is invalid, because {0}")]
pub struct InvalidPartnerShopApiKeyError(String, String);

impl PartnerShopApiKey {
    pub fn new() -> Self {
        let key = PrefixedApiKeyController::configure()
            .prefix(PARTNER_SHOP_API_KEY_PREFIX.to_owned())
            .seam_defaults()
            .finalize()
            .expect("shouldn't fail creating PrefixedApiKeyController because all required fields are set")
            .generate_key()
            .to_string();
        Self(key)
    }

    pub fn check(&self, hashed_partner_shop_api_key: &HashedPartnerShopApiKey) -> bool {
        PrefixedApiKeyController::configure()
            .prefix(PARTNER_SHOP_API_KEY_PREFIX.to_owned())
            .seam_defaults()
            .finalize()
            .expect("shouldn't fail creating PrefixedApiKeyController because all required fields are set")
            .check_hash(&self.into(), hashed_partner_shop_api_key.long_token_hash())
    }
}

impl Default for PartnerShopApiKey {
    fn default() -> Self {
        Self::new()
    }
}

impl From<PartnerShopApiKey> for String {
    fn from(value: PartnerShopApiKey) -> Self {
        value.0
    }
}

impl TryFrom<String> for PartnerShopApiKey {
    type Error = InvalidPartnerShopApiKeyError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        let _ = parse_api_key(&value)?;
        Ok(Self(value))
    }
}

impl From<&PartnerShopApiKey> for PrefixedApiKey {
    fn from(value: &PartnerShopApiKey) -> Self {
        PrefixedApiKey::from_string(&value.0)
            .expect("shouldn't fail parsing ParnterShopKey as PrefixedApiKey by construction")
    }
}

fn parse_api_key(api_key: &str) -> Result<(String, String), InvalidPartnerShopApiKeyError> {
    let (short_token, long_token) = api_key
        .strip_prefix(PARTNER_SHOP_API_KEY_PREFIX)
        .ok_or_else(|| {
            InvalidPartnerShopApiKeyError(
                api_key.to_string(),
                "it doesn't start with the required prefix '{PARTNER_SHOP_KEY_PREFIX}'".to_string(),
            )
        })?
        .strip_prefix("_")
        .ok_or_else(|| {
            InvalidPartnerShopApiKeyError(
                api_key.to_string(),
                "it contain a '_' after prefix '{PARTNER_SHOP_KEY_PREFIX}'".to_string(),
            )
        })?
        .split_once('_')
        .ok_or_else(|| {
            InvalidPartnerShopApiKeyError(
                api_key.to_string(),
                "it should contain a '_' separating the short and long token".to_string(),
            )
        })?;

    Ok((short_token.to_string(), long_token.to_string()))
}

#[derive(Debug, Clone, PartialEq)]
pub struct HashedPartnerShopApiKey {
    prefix: String,
    short_token: String,
    long_token_hash: String,
}

impl HashedPartnerShopApiKey {
    pub fn new(short_token: String, long_token_hash: String) -> Self {
        Self {
            prefix: PARTNER_SHOP_API_KEY_PREFIX.to_string(),
            short_token,
            long_token_hash,
        }
    }

    pub fn prefix(&self) -> &str {
        &self.prefix
    }

    pub fn short_token(&self) -> &str {
        &self.short_token
    }

    pub fn long_token_hash(&self) -> &str {
        &self.long_token_hash
    }
}

impl From<PartnerShopApiKey> for HashedPartnerShopApiKey {
    fn from(value: PartnerShopApiKey) -> Self {
        PrefixedApiKey::from_string(&value.0)
            .expect("shouldn't fail parsing ParnterShopKey as PrefixedApiKey by construction")
            .into()
    }
}

impl From<PrefixedApiKey> for HashedPartnerShopApiKey {
    fn from(value: PrefixedApiKey) -> Self {
        let mut digest = Sha256::new();
        Self {
            prefix: value.prefix().to_string(),
            short_token: value.short_token().to_string(),
            long_token_hash: value.long_token_hashed(&mut digest),
        }
    }
}

#[cfg(feature = "api")]
pub mod api {
    use super::{InvalidPartnerShopApiKeyError, PartnerShopApiKey};
    use aws_lambda_events::apigw::ApiGatewayV2httpRequest;
    use common::api::error::ApiError;
    use common::api::error_code::BAD_HEADER_VALUE;

    pub fn extract_api_key(
        request: &ApiGatewayV2httpRequest,
    ) -> Result<PartnerShopApiKey, ApiError> {
        let api_key_str = request
            .headers
            .get("x-api-key")
            .and_then(|v| v.to_str().ok())
            .filter(|s| !s.is_empty())
            .ok_or_else(|| {
                ApiError::unauthorized(BAD_HEADER_VALUE)
                    .with_header_field("x-api-key")
                    .with_detail("Missing or empty 'x-api-key' header.")
            })?;

        PartnerShopApiKey::try_from(api_key_str.to_string()).map_err(|err| {
            let msg = err.to_string();
            ApiError::unauthorized(BAD_HEADER_VALUE)
                .with_header_field("x-api-key")
                .with_detail(msg)
        })
    }

    impl From<InvalidPartnerShopApiKeyError> for ApiError {
        fn from(err: InvalidPartnerShopApiKeyError) -> Self {
            ApiError::unauthorized(BAD_HEADER_VALUE)
                .with_header_field("x-api-key")
                .with_detail(err.to_string())
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use aws_lambda_events::apigw::ApiGatewayV2httpRequest;
        use http::HeaderMap;

        #[test]
        fn should_extract_api_key_when_valid_header() {
            let api_key = PartnerShopApiKey::new();
            let key_str: String = api_key.clone().into();
            let mut request = ApiGatewayV2httpRequest::default();
            let mut headers = HeaderMap::new();
            headers.insert("x-api-key", key_str.parse().unwrap());
            request.headers = headers;

            let result = extract_api_key(&request);
            assert!(result.is_ok());
            assert_eq!(result.unwrap(), api_key);
        }

        #[test]
        fn should_return_401_when_api_key_header_missing() {
            let request = ApiGatewayV2httpRequest::default();
            let result = extract_api_key(&request);
            assert!(result.is_err());
            assert_eq!(result.unwrap_err().status, 401);
        }

        #[test]
        fn should_return_401_when_api_key_header_invalid() {
            let mut request = ApiGatewayV2httpRequest::default();
            let mut headers = HeaderMap::new();
            headers.insert("x-api-key", "invalid-key".parse().unwrap());
            request.headers = headers;

            let result = extract_api_key(&request);
            assert!(result.is_err());
            assert_eq!(result.unwrap_err().status, 401);
        }
    }
}

#[cfg(feature = "test-data")]
mod faker {
    use super::*;
    use fake::{Dummy, Faker, RngExt};

    impl Dummy<Faker> for PartnerShopApiKey {
        fn dummy_with_rng<R: RngExt + ?Sized>(_config: &Faker, _rng: &mut R) -> Self {
            PartnerShopApiKey::new()
        }
    }

    impl Dummy<Faker> for HashedPartnerShopApiKey {
        fn dummy_with_rng<R: RngExt + ?Sized>(_config: &Faker, _rng: &mut R) -> Self {
            PartnerShopApiKey::new().into()
        }
    }

    #[cfg(test)]
    mod tests {
        use crate::core::partner_shop_api_key::{HashedPartnerShopApiKey, PartnerShopApiKey};
        use fake::{Fake, Faker};

        #[test]
        fn should_fake_shop_partner_shop_api_key() {
            for _ in 0..100 {
                let _ = Faker.fake::<PartnerShopApiKey>();
            }
        }

        #[test]
        fn should_fake_hashed_shop_partner_shop_api_key() {
            for _ in 0..100 {
                let _ = Faker.fake::<HashedPartnerShopApiKey>();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::rstest;

    // ── PartnerShopApiKey::new / default ────────────────────────────────────

    #[test]
    fn should_start_with_prefix_when_new() {
        let key: String = PartnerShopApiKey::new().into();
        assert!(key.starts_with("aurahistoria_"));
    }

    #[test]
    fn should_have_correct_token_lengths_when_new() {
        // seam_defaults() uses 8 bytes for short (base58 → 10-11 chars)
        // and 24 bytes for long (base58 → 32-33 chars); exact length depends on leading zeros
        let key: String = PartnerShopApiKey::new().into();
        let stripped = key.strip_prefix("aurahistoria_").unwrap();
        let (short, long) = stripped.split_once('_').unwrap();
        assert!(
            (10..=11).contains(&short.len()),
            "short token should be 10-11 characters, was {}",
            short.len()
        );
        assert!(
            (32..=33).contains(&long.len()),
            "long token should be 32-33 characters, was {}",
            long.len()
        );
    }

    #[test]
    fn should_generate_unique_keys_when_called_multiple_times() {
        let keys: Vec<String> = (0..10).map(|_| PartnerShopApiKey::new().into()).collect();
        let unique: std::collections::HashSet<_> = keys.iter().collect();
        assert_eq!(unique.len(), 10);
    }

    #[test]
    fn should_produce_valid_parseable_key_when_default() {
        let key = PartnerShopApiKey::default();
        let key_str: String = key.into();
        let result = PartnerShopApiKey::try_from(key_str);
        assert!(result.is_ok());
    }

    // ── TryFrom<String> ─────────────────────────────────────────────────────

    #[rstest]
    #[case("aurahistoria_12345678901_123456789012345678901234567890123")]
    #[case("aurahistoria_abcdefghijk_abcdefghijklmnopqrstuvwxyz1234567")]
    #[trace]
    fn should_parse_valid_key_when_try_from_string(#[case] input: &str) {
        let result = PartnerShopApiKey::try_from(input.to_string());
        assert!(result.is_ok());
    }

    #[rstest]
    #[case::wrong_prefix("badprefix_12345678901_123456789012345678901234567890123")]
    #[case::no_underscore_after_prefix("aurahistoria12345678901_123456789012345678901234567890123")]
    #[case::no_token_separator("aurahistoria_12345678901123456789012345678901234567890123")]
    #[case::empty("")]
    #[trace]
    fn should_reject_invalid_key_when_try_from_string(#[case] input: &str) {
        let result = PartnerShopApiKey::try_from(input.to_string());
        assert!(result.is_err());
    }

    // ── From<PartnerShopApiKey> for String / round-trip ─────────────────────

    #[test]
    fn should_round_trip_through_string_when_from_and_try_from() {
        let key = PartnerShopApiKey::new();
        let key_str: String = key.clone().into();
        let restored = PartnerShopApiKey::try_from(key_str).unwrap();
        assert_eq!(key, restored);
    }

    // ── PartnerShopApiKey::check ─────────────────────────────────────────────

    #[test]
    fn should_return_true_when_checking_own_hash() {
        let key = PartnerShopApiKey::new();
        let hash = HashedPartnerShopApiKey::from(key.clone());
        assert!(key.check(&hash));
    }

    #[test]
    fn should_return_false_when_checking_hash_of_different_key() {
        let key1 = PartnerShopApiKey::new();
        let key2 = PartnerShopApiKey::new();
        let hash2 = HashedPartnerShopApiKey::from(key2);
        assert!(!key1.check(&hash2));
    }

    // ── Serde ────────────────────────────────────────────────────────────────

    #[test]
    fn should_serialize_as_plain_json_string_when_serializing() {
        let key = PartnerShopApiKey::new();
        let key_str: String = key.clone().into();
        let json = serde_json::to_string(&key).unwrap();
        assert_eq!(json, format!("\"{}\"", key_str));
    }

    #[test]
    fn should_round_trip_through_serde_when_serializing_and_deserializing() {
        let key = PartnerShopApiKey::new();
        let json = serde_json::to_string(&key).unwrap();
        let restored: PartnerShopApiKey = serde_json::from_str(&json).unwrap();
        assert_eq!(key, restored);
    }

    #[rstest]
    #[case::empty_string("\"\"")]
    #[case::no_prefix("\"invalid_key_value\"")]
    #[case::wrong_prefix("\"badprefix_12345678901_123456789012345678901234567890123\"")]
    #[trace]
    fn should_fail_deserializing_when_json_contains_invalid_key(#[case] json: &str) {
        let result = serde_json::from_str::<PartnerShopApiKey>(json);
        assert!(result.is_err());
    }

    // ── From<&PartnerShopApiKey> for PrefixedApiKey ──────────────────────────

    #[test]
    fn should_preserve_prefix_and_short_token_when_converting_to_prefixed_api_key() {
        let key = PartnerShopApiKey::new();
        let key_str: String = key.clone().into();
        let stripped = key_str.strip_prefix("aurahistoria_").unwrap();
        let (expected_short, _) = stripped.split_once('_').unwrap();

        let prefixed: PrefixedApiKey = (&key).into();

        assert_eq!(prefixed.prefix(), "aurahistoria");
        assert_eq!(prefixed.short_token(), expected_short);
    }

    // ── HashedPartnerShopApiKey::new and accessors ───────────────────────────

    #[test]
    fn should_store_all_fields_correctly_when_creating_via_new() {
        let short = "ABCDEFGH".to_string();
        let hash = "somehashvalue".to_string();

        let hashed = HashedPartnerShopApiKey::new(short.clone(), hash.clone());

        assert_eq!(hashed.prefix(), PARTNER_SHOP_API_KEY_PREFIX);
        assert_eq!(hashed.short_token(), short);
        assert_eq!(hashed.long_token_hash(), hash);
    }

    // ── From<PartnerShopApiKey> for HashedPartnerShopApiKey ──────────────────

    #[test]
    fn should_produce_sha256_hash_when_converting_from_partner_shop_api_key() {
        let key = PartnerShopApiKey::new();
        let key_str: String = key.clone().into();
        let stripped = key_str.strip_prefix("aurahistoria_").unwrap();
        let (expected_short, _) = stripped.split_once('_').unwrap();

        let hashed = HashedPartnerShopApiKey::from(key);

        assert_eq!(hashed.prefix(), PARTNER_SHOP_API_KEY_PREFIX);
        assert_eq!(hashed.short_token(), expected_short);
        assert_eq!(
            hashed.long_token_hash().len(),
            64,
            "SHA-256 hex digest should be 64 characters"
        );
    }

    #[test]
    fn should_produce_equal_hashes_when_converting_same_key_twice() {
        let key = PartnerShopApiKey::new();
        let hash1 = HashedPartnerShopApiKey::from(key.clone());
        let hash2 = HashedPartnerShopApiKey::from(key);
        assert_eq!(hash1, hash2);
    }

    #[test]
    fn should_produce_different_hashes_when_converting_different_keys() {
        let key1 = PartnerShopApiKey::new();
        let key2 = PartnerShopApiKey::new();
        let hash1 = HashedPartnerShopApiKey::from(key1);
        let hash2 = HashedPartnerShopApiKey::from(key2);
        assert_ne!(hash1, hash2);
    }

    // ── From<PrefixedApiKey> for HashedPartnerShopApiKey ────────────────────

    #[test]
    fn should_produce_same_result_when_converting_from_prefixed_api_key_as_from_partner_shop_api_key()
     {
        let key = PartnerShopApiKey::new();
        let prefixed: PrefixedApiKey = (&key).into();

        let hash_from_key = HashedPartnerShopApiKey::from(key);
        let hash_from_prefixed = HashedPartnerShopApiKey::from(prefixed);

        assert_eq!(hash_from_key, hash_from_prefixed);
    }

    #[test]
    fn should_set_correct_prefix_when_converting_from_prefixed_api_key() {
        let key = PartnerShopApiKey::new();
        let prefixed: PrefixedApiKey = (&key).into();
        let hashed = HashedPartnerShopApiKey::from(prefixed);
        assert_eq!(hashed.prefix(), PARTNER_SHOP_API_KEY_PREFIX);
    }

    // ── InvalidPartnerShopApiKeyError ────────────────────────────────────────

    #[test]
    fn should_include_input_value_in_error_message_when_parsing_fails() {
        let bad_key = "totally_wrong_format";
        let err = PartnerShopApiKey::try_from(bad_key.to_string()).unwrap_err();
        assert!(err.to_string().contains(bad_key));
    }

    #[rstest]
    #[case::wrong_prefix("wrongprefix_12345678901_123456789012345678901234567890123")]
    #[case::no_separator("aurahistoria_12345678901123456789012345678901234567890123")]
    #[trace]
    fn should_include_bad_key_in_error_message_for_each_validation_branch(#[case] bad_key: &str) {
        let err = PartnerShopApiKey::try_from(bad_key.to_string()).unwrap_err();
        assert!(err.to_string().contains(bad_key));
    }
}

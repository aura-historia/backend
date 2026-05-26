use common::{string_newtype, user_id::UserId, uuid_v7_newtype};
use prefixed_api_key::{
    PrefixedApiKey, PrefixedApiKeyController,
    sha2::{Digest, Sha256},
};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use time::OffsetDateTime;

uuid_v7_newtype!(AccessTokenId);
string_newtype!(AccessTokenName, max_length(128));

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Scope {
    ShopsManage,
    ProductsWrite,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AccessToken {
    pub id: AccessTokenId,
    pub hashed_token: HashedRawAccessToken,
    pub user_id: UserId,
    pub name: AccessTokenName,
    pub scopes: HashSet<Scope>,
    pub expires: Option<OffsetDateTime>,
    pub created: OffsetDateTime,
    pub updated: OffsetDateTime,
}

impl AccessToken {
    pub fn is_expired(&self) -> bool {
        if let Some(expires) = self.expires {
            expires < OffsetDateTime::now_utc()
        } else {
            false
        }
    }

    pub fn has_scope(&self, scope: Scope) -> bool {
        self.scopes.contains(&scope)
    }
}

const AURA_HISTORIA_ACCESS_TOKEN_PREFIX: &str = "aurahistoria";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(into = "String", try_from = "String")]
pub struct RawAccessToken(String);

#[derive(Debug, Clone, thiserror::Error)]
#[error("RawAccessToken '{0}' is invalid, because {0}")]
pub struct InvalidRawAccessTokenError(String, String);

impl RawAccessToken {
    pub fn new() -> Self {
        let key = PrefixedApiKeyController::configure()
            .prefix(AURA_HISTORIA_ACCESS_TOKEN_PREFIX.to_owned())
            .seam_defaults()
            .finalize()
            .expect("shouldn't fail creating PrefixedApiKeyController because all required fields are set")
            .generate_key()
            .to_string();
        Self(key)
    }

    pub fn check(&self, hashed_aura_historia_api_key: &HashedRawAccessToken) -> bool {
        PrefixedApiKeyController::configure()
            .prefix(AURA_HISTORIA_ACCESS_TOKEN_PREFIX.to_owned())
            .seam_defaults()
            .finalize()
            .expect("shouldn't fail creating PrefixedApiKeyController because all required fields are set")
            .check_hash(&self.into(), hashed_aura_historia_api_key.long_token_hash())
    }
}

impl Default for RawAccessToken {
    fn default() -> Self {
        Self::new()
    }
}

impl From<RawAccessToken> for String {
    fn from(value: RawAccessToken) -> Self {
        value.0
    }
}

impl TryFrom<String> for RawAccessToken {
    type Error = InvalidRawAccessTokenError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        let _ = parse_api_key(&value)?;
        Ok(Self(value))
    }
}

impl From<&RawAccessToken> for PrefixedApiKey {
    fn from(value: &RawAccessToken) -> Self {
        PrefixedApiKey::from_string(&value.0)
            .expect("shouldn't fail parsing RawAccessToken as PrefixedApiKey by construction")
    }
}

fn parse_api_key(api_key: &str) -> Result<(String, String), InvalidRawAccessTokenError> {
    let (short_token, long_token) = api_key
        .strip_prefix(AURA_HISTORIA_ACCESS_TOKEN_PREFIX)
        .ok_or_else(|| {
            InvalidRawAccessTokenError(
                api_key.to_string(),
                "it doesn't start with the required prefix '{AURA_HISTORIA_ACCESS_TOKEN_PREFIX}'"
                    .to_string(),
            )
        })?
        .strip_prefix("_")
        .ok_or_else(|| {
            InvalidRawAccessTokenError(
                api_key.to_string(),
                "it should contain a '_' after prefix '{AURA_HISTORIA_ACCESS_TOKEN_PREFIX}'"
                    .to_string(),
            )
        })?
        .split_once('_')
        .ok_or_else(|| {
            InvalidRawAccessTokenError(
                api_key.to_string(),
                "it should contain a '_' separating the short and long token".to_string(),
            )
        })?;

    Ok((short_token.to_string(), long_token.to_string()))
}

#[derive(Debug, Clone, PartialEq)]
pub struct HashedRawAccessToken {
    prefix: String,
    short_token: String,
    long_token_hash: String,
}

impl HashedRawAccessToken {
    pub fn new(short_token: String, long_token_hash: String) -> Self {
        Self {
            prefix: AURA_HISTORIA_ACCESS_TOKEN_PREFIX.to_string(),
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

impl From<RawAccessToken> for HashedRawAccessToken {
    fn from(value: RawAccessToken) -> Self {
        PrefixedApiKey::from_string(&value.0)
            .expect("shouldn't fail parsing RawAccessToken as PrefixedApiKey by construction")
            .into()
    }
}

impl From<PrefixedApiKey> for HashedRawAccessToken {
    fn from(value: PrefixedApiKey) -> Self {
        let mut digest = Sha256::new();
        Self {
            prefix: value.prefix().to_string(),
            short_token: value.short_token().to_string(),
            long_token_hash: value.long_token_hashed(&mut digest),
        }
    }
}

#[cfg(feature = "test-data")]
mod faker {
    use super::*;
    use fake::{Dummy, Fake, Faker, RngExt};

    impl Dummy<Faker> for RawAccessToken {
        fn dummy_with_rng<R: RngExt + ?Sized>(_config: &Faker, _rng: &mut R) -> Self {
            RawAccessToken::new()
        }
    }

    impl Dummy<Faker> for HashedRawAccessToken {
        fn dummy_with_rng<R: RngExt + ?Sized>(_config: &Faker, _rng: &mut R) -> Self {
            RawAccessToken::new().into()
        }
    }

    impl Dummy<Faker> for AccessToken {
        fn dummy_with_rng<R: RngExt + ?Sized>(config: &Faker, rng: &mut R) -> Self {
            AccessToken {
                id: config.fake_with_rng(rng),
                hashed_token: config.fake_with_rng(rng),
                user_id: config.fake_with_rng(rng),
                name: config.fake_with_rng(rng),
                scopes: [Scope::ProductsWrite].into(),
                expires: None,
                created: OffsetDateTime::now_utc(),
                updated: OffsetDateTime::now_utc(),
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use crate::core::access_token::{HashedRawAccessToken, RawAccessToken};
        use fake::{Fake, Faker};

        #[test]
        fn should_fake_access_token() {
            for _ in 0..100 {
                let _ = Faker.fake::<RawAccessToken>();
            }
        }

        #[test]
        fn should_fake_hashed_access_token() {
            for _ in 0..100 {
                let _ = Faker.fake::<HashedRawAccessToken>();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::rstest;

    // ── RawAccessToken::new / default ────────────────────────────────────

    #[test]
    fn should_start_with_prefix_when_new() {
        let key: String = RawAccessToken::new().into();
        assert!(key.starts_with("aurahistoria_"));
    }

    #[test]
    fn should_have_correct_token_lengths_when_new() {
        // seam_defaults() uses 8 bytes for short (base58 → 10-11 chars)
        // and 24 bytes for long (base58 → 32-33 chars); exact length depends on leading zeros
        let key: String = RawAccessToken::new().into();
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
        let keys: Vec<String> = (0..10).map(|_| RawAccessToken::new().into()).collect();
        let unique: std::collections::HashSet<_> = keys.iter().collect();
        assert_eq!(unique.len(), 10);
    }

    #[test]
    fn should_produce_valid_parseable_key_when_default() {
        let key = RawAccessToken::default();
        let key_str: String = key.into();
        let result = RawAccessToken::try_from(key_str);
        assert!(result.is_ok());
    }

    // ── TryFrom<String> ─────────────────────────────────────────────────────

    #[rstest]
    #[case("aurahistoria_12345678901_123456789012345678901234567890123")]
    #[case("aurahistoria_abcdefghijk_abcdefghijklmnopqrstuvwxyz1234567")]
    #[trace]
    fn should_parse_valid_key_when_try_from_string(#[case] input: &str) {
        let result = RawAccessToken::try_from(input.to_string());
        assert!(result.is_ok());
    }

    #[rstest]
    #[case::wrong_prefix("badprefix_12345678901_123456789012345678901234567890123")]
    #[case::no_underscore_after_prefix("aurahistoria12345678901_123456789012345678901234567890123")]
    #[case::no_token_separator("aurahistoria_12345678901123456789012345678901234567890123")]
    #[case::empty("")]
    #[trace]
    fn should_reject_invalid_key_when_try_from_string(#[case] input: &str) {
        let result = RawAccessToken::try_from(input.to_string());
        assert!(result.is_err());
    }

    // ── From<RawAccessToken> for String / round-trip ─────────────────────

    #[test]
    fn should_round_trip_through_string_when_from_and_try_from() {
        let key = RawAccessToken::new();
        let key_str: String = key.clone().into();
        let restored = RawAccessToken::try_from(key_str).unwrap();
        assert_eq!(key, restored);
    }

    // ── RawAccessToken::check ─────────────────────────────────────────────

    #[test]
    fn should_return_true_when_checking_own_hash() {
        let key = RawAccessToken::new();
        let hash = HashedRawAccessToken::from(key.clone());
        assert!(key.check(&hash));
    }

    #[test]
    fn should_return_false_when_checking_hash_of_different_key() {
        let key1 = RawAccessToken::new();
        let key2 = RawAccessToken::new();
        let hash2 = HashedRawAccessToken::from(key2);
        assert!(!key1.check(&hash2));
    }

    // ── Serde ────────────────────────────────────────────────────────────────

    #[test]
    fn should_serialize_as_plain_json_string_when_serializing() {
        let key = RawAccessToken::new();
        let key_str: String = key.clone().into();
        let json = serde_json::to_string(&key).unwrap();
        assert_eq!(json, format!("\"{}\"", key_str));
    }

    #[test]
    fn should_round_trip_through_serde_when_serializing_and_deserializing() {
        let key = RawAccessToken::new();
        let json = serde_json::to_string(&key).unwrap();
        let restored: RawAccessToken = serde_json::from_str(&json).unwrap();
        assert_eq!(key, restored);
    }

    #[rstest]
    #[case::empty_string("\"\"")]
    #[case::no_prefix("\"invalid_key_value\"")]
    #[case::wrong_prefix("\"badprefix_12345678901_123456789012345678901234567890123\"")]
    #[trace]
    fn should_fail_deserializing_when_json_contains_invalid_key(#[case] json: &str) {
        let result = serde_json::from_str::<RawAccessToken>(json);
        assert!(result.is_err());
    }

    // ── From<&RawAccessToken> for PrefixedApiKey ──────────────────────────

    #[test]
    fn should_preserve_prefix_and_short_token_when_converting_to_prefixed_api_key() {
        let key = RawAccessToken::new();
        let key_str: String = key.clone().into();
        let stripped = key_str.strip_prefix("aurahistoria_").unwrap();
        let (expected_short, _) = stripped.split_once('_').unwrap();

        let prefixed: PrefixedApiKey = (&key).into();

        assert_eq!(prefixed.prefix(), "aurahistoria");
        assert_eq!(prefixed.short_token(), expected_short);
    }

    // ── HashedRawAccessToken::new and accessors ───────────────────────────

    #[test]
    fn should_store_all_fields_correctly_when_creating_via_new() {
        let short = "ABCDEFGH".to_string();
        let hash = "somehashvalue".to_string();

        let hashed = HashedRawAccessToken::new(short.clone(), hash.clone());

        assert_eq!(hashed.prefix(), AURA_HISTORIA_ACCESS_TOKEN_PREFIX);
        assert_eq!(hashed.short_token(), short);
        assert_eq!(hashed.long_token_hash(), hash);
    }

    // ── From<RawAccessToken> for HashedRawAccessToken ──────────────────

    #[test]
    fn should_produce_sha256_hash_when_converting_from_aura_historia_api_key() {
        let key = RawAccessToken::new();
        let key_str: String = key.clone().into();
        let stripped = key_str.strip_prefix("aurahistoria_").unwrap();
        let (expected_short, _) = stripped.split_once('_').unwrap();

        let hashed = HashedRawAccessToken::from(key);

        assert_eq!(hashed.prefix(), AURA_HISTORIA_ACCESS_TOKEN_PREFIX);
        assert_eq!(hashed.short_token(), expected_short);
        assert_eq!(
            hashed.long_token_hash().len(),
            64,
            "SHA-256 hex digest should be 64 characters"
        );
    }

    #[test]
    fn should_produce_equal_hashes_when_converting_same_key_twice() {
        let key = RawAccessToken::new();
        let hash1 = HashedRawAccessToken::from(key.clone());
        let hash2 = HashedRawAccessToken::from(key);
        assert_eq!(hash1, hash2);
    }

    #[test]
    fn should_produce_different_hashes_when_converting_different_keys() {
        let key1 = RawAccessToken::new();
        let key2 = RawAccessToken::new();
        let hash1 = HashedRawAccessToken::from(key1);
        let hash2 = HashedRawAccessToken::from(key2);
        assert_ne!(hash1, hash2);
    }

    // ── From<PrefixedApiKey> for HashedRawAccessToken ────────────────────

    #[test]
    fn should_produce_same_result_when_converting_from_prefixed_api_key_as_from_aura_historia_api_key()
     {
        let key = RawAccessToken::new();
        let prefixed: PrefixedApiKey = (&key).into();

        let hash_from_key = HashedRawAccessToken::from(key);
        let hash_from_prefixed = HashedRawAccessToken::from(prefixed);

        assert_eq!(hash_from_key, hash_from_prefixed);
    }

    #[test]
    fn should_set_correct_prefix_when_converting_from_prefixed_api_key() {
        let key = RawAccessToken::new();
        let prefixed: PrefixedApiKey = (&key).into();
        let hashed = HashedRawAccessToken::from(prefixed);
        assert_eq!(hashed.prefix(), AURA_HISTORIA_ACCESS_TOKEN_PREFIX);
    }

    // ── InvalidRawAccessTokenError ────────────────────────────────────────

    #[test]
    fn should_include_input_value_in_error_message_when_parsing_fails() {
        let bad_key = "totally_wrong_format";
        let err = RawAccessToken::try_from(bad_key.to_string()).unwrap_err();
        assert!(err.to_string().contains(bad_key));
    }

    #[rstest]
    #[case::wrong_prefix("wrongprefix_12345678901_123456789012345678901234567890123")]
    #[case::no_separator("aurahistoria_12345678901123456789012345678901234567890123")]
    #[trace]
    fn should_include_bad_key_in_error_message_for_each_validation_branch(#[case] bad_key: &str) {
        let err = RawAccessToken::try_from(bad_key.to_string()).unwrap_err();
        assert!(err.to_string().contains(bad_key));
    }
}

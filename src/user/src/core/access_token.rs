use common::{string_newtype, user_id::UserId, uuid_v7_newtype};
use prefixed_api_key::{
    PrefixedApiKey, PrefixedApiKeyController,
    sha2::{Digest, Sha256},
};
use std::collections::HashSet;
use std::marker::PhantomData;
use time::OffsetDateTime;

uuid_v7_newtype!(AccessTokenId);
string_newtype!(AccessTokenName, max_length(128));

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
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
    pub origin: AccessTokenOrigin,
    pub expires: Option<OffsetDateTime>,
    pub created: OffsetDateTime,
    pub updated: OffsetDateTime,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AccessTokenOrigin {
    User,
    OAuth { client_id: String },
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

// ── TokenPrefix trait and prefix types ─────────────────────────────────────

pub trait TokenPrefix: std::fmt::Debug + Clone + PartialEq + Send + Sync + 'static {
    const PREFIX: &'static str;
}

#[derive(Debug, Clone, PartialEq)]
pub struct AccessTokenPrefix;

impl TokenPrefix for AccessTokenPrefix {
    const PREFIX: &'static str = "aurahistoria_accesstoken";
}

#[derive(Debug, Clone, PartialEq)]
pub struct OAuthClientSecretPrefix;

impl TokenPrefix for OAuthClientSecretPrefix {
    const PREFIX: &'static str = "aurahistoria_oauth_client_secret";
}

// ── Type aliases ────────────────────────────────────────────────────────────

pub type RawAccessToken = RawToken<AccessTokenPrefix>;
pub type RawOAuthClientSecret = RawToken<OAuthClientSecretPrefix>;
pub type HashedRawAccessToken = HashedRawToken<AccessTokenPrefix>;
pub type HashedRawOAuthClientSecret = HashedRawToken<OAuthClientSecretPrefix>;

// ── Error type ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone, thiserror::Error)]
#[error("RawToken '{0}' is invalid, because {1}")]
pub struct InvalidRawTokenError(String, String);

/// Backward-compatibility alias.
pub type InvalidRawAccessTokenError = InvalidRawTokenError;

// ── RawToken<P> ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
pub struct RawToken<P: TokenPrefix> {
    value: String,
    _marker: PhantomData<P>,
}

impl<P: TokenPrefix> RawToken<P> {
    pub fn new() -> Self {
        let key = PrefixedApiKeyController::configure()
            .prefix(P::PREFIX.to_owned())
            .seam_defaults()
            .finalize()
            .expect("shouldn't fail creating PrefixedApiKeyController because all required fields are set")
            .generate_key()
            .to_string();
        Self {
            value: key,
            _marker: PhantomData,
        }
    }

    pub fn check(&self, hashed: &HashedRawToken<P>) -> bool {
        PrefixedApiKeyController::configure()
            .prefix(P::PREFIX.to_owned())
            .seam_defaults()
            .finalize()
            .expect("shouldn't fail creating PrefixedApiKeyController because all required fields are set")
            .check_hash(&self.into(), hashed.long_token_hash())
    }
}

impl<P: TokenPrefix> Default for RawToken<P> {
    fn default() -> Self {
        Self::new()
    }
}

impl<P: TokenPrefix> From<RawToken<P>> for String {
    fn from(value: RawToken<P>) -> Self {
        value.value
    }
}

impl<P: TokenPrefix> TryFrom<String> for RawToken<P> {
    type Error = InvalidRawTokenError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        let _ = parse_token(&value, P::PREFIX)?;
        Ok(Self {
            value,
            _marker: PhantomData,
        })
    }
}

#[cfg(feature = "data")]
pub mod api {
    use crate::core::access_token::{AccessTokenId, RawAccessToken};
    use common::{
        api::{
            error::ApiError,
            error_code::{BAD_HEADER_VALUE, BAD_PATH_PARAMETER_VALUE, INVALID_UUID},
        },
        error::missing_field::MissingRequiredField,
    };
    use http::{HeaderMap, header::AUTHORIZATION};
    use std::collections::HashMap;

    #[derive(Debug, thiserror::Error)]
    pub enum ExtractBearerTokenError {
        #[error("Invalid authorization header value: '{0}'")]
        InvalidHeaderValue(#[from] http::header::ToStrError),
        #[error("Token in authorization header is not a valid bearer token")]
        InvalidBearerTokenFormat,
    }

    impl From<ExtractBearerTokenError> for ApiError {
        fn from(value: ExtractBearerTokenError) -> Self {
            match value {
                ExtractBearerTokenError::InvalidHeaderValue(err) => {
                    let msg = err.to_string();
                    ApiError::bad_request(BAD_HEADER_VALUE, Box::new(err))
                        .with_header_field(AUTHORIZATION.as_str())
                        .with_detail(msg)
                }
                err @ ExtractBearerTokenError::InvalidBearerTokenFormat => {
                    let msg = err.to_string();
                    ApiError::bad_request(BAD_HEADER_VALUE, Box::new(err))
                        .with_header_field(AUTHORIZATION.as_str())
                        .with_detail(msg)
                }
            }
        }
    }

    pub fn extract_bearer_token(
        headers: &HeaderMap,
    ) -> Result<Option<String>, ExtractBearerTokenError> {
        let authorization = headers
            .get(AUTHORIZATION)
            .map(|value| value.to_str())
            .transpose()?;

        match authorization {
            None => Ok(None),
            Some(value) => value
                .strip_prefix("Bearer ")
                .map(ToOwned::to_owned)
                .ok_or(ExtractBearerTokenError::InvalidBearerTokenFormat)
                .map(Some),
        }
    }

    pub fn extract_bearer_access_token(
        headers: &HeaderMap,
    ) -> Result<Option<RawAccessToken>, ApiError> {
        extract_bearer_token(headers)?
            .map(|value| {
                RawAccessToken::try_from(value).map_err(|err| {
                    ApiError::unauthorized(common::api::error_code::UNAUTHORIZED)
                        .with_header_field(AUTHORIZATION.as_str())
                        .with_detail(err.to_string())
                })
            })
            .transpose()
    }

    pub fn extract_access_token_id_path(
        path_params: &HashMap<String, String>,
    ) -> Result<AccessTokenId, ApiError> {
        path_params
            .get("accessTokenId")
            .map(AccessTokenId::try_from)
            .transpose()
            .map_err(|err| {
                let msg = err.to_string();
                ApiError::bad_request(INVALID_UUID, Box::new(err))
                    .with_path_field("accessTokenId")
                    .with_detail(msg)
            })?
            .ok_or(
                ApiError::bad_request(
                    BAD_PATH_PARAMETER_VALUE,
                    Box::new(MissingRequiredField::new("accessTokenId")),
                )
                .with_path_field("accessTokenId")
                .with_detail("Missing field 'accessTokenId'."),
            )
    }
}

// `PrefixedApiKey::from_string` requires exactly three `_`-delimited parts, so
// it cannot handle prefixes that themselves contain underscores.  We parse with
// `parse_token` instead and construct via `PrefixedApiKey::new`.
impl<P: TokenPrefix> From<&RawToken<P>> for PrefixedApiKey {
    fn from(value: &RawToken<P>) -> Self {
        let (short_token, long_token) = parse_token(&value.value, P::PREFIX)
            .expect("shouldn't fail parsing RawToken as PrefixedApiKey by construction");
        PrefixedApiKey::new(P::PREFIX.to_owned(), short_token, long_token)
    }
}

fn parse_token(token: &str, prefix: &str) -> Result<(String, String), InvalidRawTokenError> {
    let (short_token, long_token) = token
        .strip_prefix(prefix)
        .ok_or_else(|| {
            InvalidRawTokenError(
                token.to_string(),
                format!("it doesn't start with the required prefix '{prefix}'"),
            )
        })?
        .strip_prefix('_')
        .ok_or_else(|| {
            InvalidRawTokenError(
                token.to_string(),
                format!("it should contain a '_' after prefix '{prefix}'"),
            )
        })?
        .split_once('_')
        .ok_or_else(|| {
            InvalidRawTokenError(
                token.to_string(),
                "it should contain a '_' separating the short and long token".to_string(),
            )
        })?;

    Ok((short_token.to_string(), long_token.to_string()))
}

// ── HashedRawToken<P> ────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
pub struct HashedRawToken<P: TokenPrefix> {
    short_token: String,
    long_token_hash: String,
    _marker: PhantomData<P>,
}

impl<P: TokenPrefix> std::fmt::Display for HashedRawToken<P> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}_{}_****", P::PREFIX, self.short_token)
    }
}

impl<P: TokenPrefix> HashedRawToken<P> {
    pub fn new(short_token: String, long_token_hash: String) -> Self {
        Self {
            short_token,
            long_token_hash,
            _marker: PhantomData,
        }
    }

    pub fn prefix(&self) -> &'static str {
        P::PREFIX
    }

    pub fn short_token(&self) -> &str {
        &self.short_token
    }

    pub fn long_token_hash(&self) -> &str {
        &self.long_token_hash
    }
}

impl<P: TokenPrefix> From<RawToken<P>> for HashedRawToken<P> {
    fn from(value: RawToken<P>) -> Self {
        let pak: PrefixedApiKey = (&value).into();
        pak.into()
    }
}

impl<P: TokenPrefix> From<PrefixedApiKey> for HashedRawToken<P> {
    fn from(value: PrefixedApiKey) -> Self {
        let mut digest = Sha256::new();
        Self {
            short_token: value.short_token().to_string(),
            long_token_hash: value.long_token_hashed(&mut digest),
            _marker: PhantomData,
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

    impl Dummy<Faker> for RawOAuthClientSecret {
        fn dummy_with_rng<R: RngExt + ?Sized>(_config: &Faker, _rng: &mut R) -> Self {
            RawOAuthClientSecret::new()
        }
    }

    impl Dummy<Faker> for HashedRawAccessToken {
        fn dummy_with_rng<R: RngExt + ?Sized>(_config: &Faker, _rng: &mut R) -> Self {
            RawAccessToken::new().into()
        }
    }

    impl Dummy<Faker> for HashedRawOAuthClientSecret {
        fn dummy_with_rng<R: RngExt + ?Sized>(_config: &Faker, _rng: &mut R) -> Self {
            RawOAuthClientSecret::new().into()
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
                origin: AccessTokenOrigin::User,
                expires: None,
                created: OffsetDateTime::now_utc(),
                updated: OffsetDateTime::now_utc(),
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use crate::core::access_token::{
            HashedRawAccessToken, HashedRawOAuthClientSecret, RawAccessToken, RawOAuthClientSecret,
        };
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

        #[test]
        fn should_fake_oauth_client_secret() {
            for _ in 0..100 {
                let _ = Faker.fake::<RawOAuthClientSecret>();
            }
        }

        #[test]
        fn should_fake_hashed_oauth_client_secret() {
            for _ in 0..100 {
                let _ = Faker.fake::<HashedRawOAuthClientSecret>();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::rstest;

    // ── Prefix starts ─────────────────────────────────────────────────────

    #[test]
    fn should_start_with_access_token_prefix_when_new_for_access_token() {
        let key: String = RawAccessToken::new().into();
        assert!(key.starts_with("aurahistoria_accesstoken_"));
    }

    #[test]
    fn should_start_with_oauth_secret_prefix_when_new_for_oauth_secret() {
        let key: String = RawOAuthClientSecret::new().into();
        assert!(key.starts_with("aurahistoria_oauth_client_secret_"));
    }

    // ── Token lengths ─────────────────────────────────────────────────────

    #[test]
    fn should_have_correct_token_lengths_when_new_for_access_token() {
        // seam_defaults() uses 8 bytes for short (base58 → 10-11 chars)
        // and 24 bytes for long (base58 → 32-33 chars); exact length depends on leading zeros
        let key: String = RawAccessToken::new().into();
        let stripped = key.strip_prefix("aurahistoria_accesstoken_").unwrap();
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
    fn should_have_correct_token_lengths_when_new_for_oauth_secret() {
        let key: String = RawOAuthClientSecret::new().into();
        let stripped = key
            .strip_prefix("aurahistoria_oauth_client_secret_")
            .unwrap();
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

    // ── Uniqueness ────────────────────────────────────────────────────────

    #[test]
    fn should_generate_unique_keys_when_called_multiple_times_for_access_token() {
        let keys: Vec<String> = (0..10).map(|_| RawAccessToken::new().into()).collect();
        let unique: std::collections::HashSet<_> = keys.iter().collect();
        assert_eq!(unique.len(), 10);
    }

    #[test]
    fn should_generate_unique_keys_when_called_multiple_times_for_oauth_secret() {
        let keys: Vec<String> = (0..10)
            .map(|_| RawOAuthClientSecret::new().into())
            .collect();
        let unique: std::collections::HashSet<_> = keys.iter().collect();
        assert_eq!(unique.len(), 10);
    }

    // ── Default ───────────────────────────────────────────────────────────

    #[test]
    fn should_produce_valid_parseable_key_when_default_for_access_token() {
        let key = RawAccessToken::default();
        let key_str: String = key.into();
        let result = RawAccessToken::try_from(key_str);
        assert!(result.is_ok());
    }

    // ── TryFrom<String>: valid cases ──────────────────────────────────────

    #[rstest]
    #[case("aurahistoria_accesstoken_12345678901_123456789012345678901234567890123")]
    #[case("aurahistoria_accesstoken_abcdefghijk_abcdefghijklmnopqrstuvwxyz1234567")]
    #[trace]
    fn should_parse_valid_key_when_try_from_string_for_access_token(#[case] input: &str) {
        let result = RawAccessToken::try_from(input.to_string());
        assert!(result.is_ok());
    }

    #[rstest]
    #[case("aurahistoria_oauth_client_secret_12345678901_123456789012345678901234567890123")]
    #[case("aurahistoria_oauth_client_secret_abcdefghijk_abcdefghijklmnopqrstuvwxyz1234567")]
    #[trace]
    fn should_parse_valid_key_when_try_from_string_for_oauth_secret(#[case] input: &str) {
        let result = RawOAuthClientSecret::try_from(input.to_string());
        assert!(result.is_ok());
    }

    // ── TryFrom<String>: invalid / rejected cases ─────────────────────────

    #[rstest]
    #[case::wrong_prefix("badprefix_12345678901_123456789012345678901234567890123")]
    #[case::old_prefix("aurahistoria_12345678901_123456789012345678901234567890123")]
    #[case::cross_type_oauth_prefix(
        "aurahistoria_oauth_client_secret_12345678901_123456789012345678901234567890123"
    )]
    #[case::no_underscore_after_prefix(
        "aurahistoria_accesstoken12345678901_123456789012345678901234567890123"
    )]
    #[case::no_token_separator(
        "aurahistoria_accesstoken_12345678901123456789012345678901234567890123"
    )]
    #[case::empty("")]
    #[trace]
    fn should_reject_invalid_key_when_try_from_string_for_access_token(#[case] input: &str) {
        let result = RawAccessToken::try_from(input.to_string());
        assert!(result.is_err());
    }

    #[rstest]
    #[case::wrong_prefix("badprefix_12345678901_123456789012345678901234567890123")]
    #[case::old_prefix("aurahistoria_12345678901_123456789012345678901234567890123")]
    #[case::cross_type_access_token_prefix(
        "aurahistoria_accesstoken_12345678901_123456789012345678901234567890123"
    )]
    #[case::no_token_separator(
        "aurahistoria_oauth_client_secret_12345678901123456789012345678901234567890123"
    )]
    #[case::empty("")]
    #[trace]
    fn should_reject_invalid_key_when_try_from_string_for_oauth_secret(#[case] input: &str) {
        let result = RawOAuthClientSecret::try_from(input.to_string());
        assert!(result.is_err());
    }

    // ── Round-trip ────────────────────────────────────────────────────────

    #[test]
    fn should_round_trip_through_string_when_from_and_try_from_for_access_token() {
        let key = RawAccessToken::new();
        let key_str: String = key.clone().into();
        let restored = RawAccessToken::try_from(key_str).unwrap();
        assert_eq!(key, restored);
    }

    #[test]
    fn should_round_trip_through_string_when_from_and_try_from_for_oauth_secret() {
        let key = RawOAuthClientSecret::new();
        let key_str: String = key.clone().into();
        let restored = RawOAuthClientSecret::try_from(key_str).unwrap();
        assert_eq!(key, restored);
    }

    // ── check() ───────────────────────────────────────────────────────────

    #[test]
    fn should_return_true_when_checking_own_hash_for_access_token() {
        let key = RawAccessToken::new();
        let hash = HashedRawAccessToken::from(key.clone());
        assert!(key.check(&hash));
    }

    #[test]
    fn should_return_false_when_checking_hash_of_different_key_for_access_token() {
        let key1 = RawAccessToken::new();
        let key2 = RawAccessToken::new();
        let hash2 = HashedRawAccessToken::from(key2);
        assert!(!key1.check(&hash2));
    }

    #[test]
    fn should_return_true_when_checking_own_hash_for_oauth_secret() {
        let key = RawOAuthClientSecret::new();
        let hash = HashedRawOAuthClientSecret::from(key.clone());
        assert!(key.check(&hash));
    }

    #[test]
    fn should_return_false_when_checking_hash_of_different_key_for_oauth_secret() {
        let key1 = RawOAuthClientSecret::new();
        let key2 = RawOAuthClientSecret::new();
        let hash2 = HashedRawOAuthClientSecret::from(key2);
        assert!(!key1.check(&hash2));
    }

    // ── From<&RawToken<P>> for PrefixedApiKey ─────────────────────────────

    #[test]
    fn should_preserve_prefix_and_short_token_when_converting_to_prefixed_api_key() {
        let key = RawAccessToken::new();
        let key_str: String = key.clone().into();
        let stripped = key_str.strip_prefix("aurahistoria_accesstoken_").unwrap();
        let (expected_short, _) = stripped.split_once('_').unwrap();

        let prefixed: PrefixedApiKey = (&key).into();

        assert_eq!(prefixed.prefix(), "aurahistoria_accesstoken");
        assert_eq!(prefixed.short_token(), expected_short);
    }

    // ── HashedRawToken::new and accessors ─────────────────────────────────

    #[test]
    fn should_store_all_fields_correctly_when_creating_via_new_for_access_token() {
        let short = "ABCDEFGH".to_string();
        let hash = "somehashvalue".to_string();

        let hashed = HashedRawAccessToken::new(short.clone(), hash.clone());

        assert_eq!(hashed.prefix(), AccessTokenPrefix::PREFIX);
        assert_eq!(hashed.short_token(), short);
        assert_eq!(hashed.long_token_hash(), hash);
    }

    #[test]
    fn should_store_all_fields_correctly_when_creating_via_new_for_oauth_secret() {
        let short = "ABCDEFGH".to_string();
        let hash = "somehashvalue".to_string();

        let hashed = HashedRawOAuthClientSecret::new(short.clone(), hash.clone());

        assert_eq!(hashed.prefix(), OAuthClientSecretPrefix::PREFIX);
        assert_eq!(hashed.short_token(), short);
        assert_eq!(hashed.long_token_hash(), hash);
    }

    // ── From<RawToken<P>> for HashedRawToken<P> ───────────────────────────

    #[test]
    fn should_produce_sha256_hash_when_converting_from_raw_token_for_access_token() {
        let key = RawAccessToken::new();
        let key_str: String = key.clone().into();
        let stripped = key_str.strip_prefix("aurahistoria_accesstoken_").unwrap();
        let (expected_short, _) = stripped.split_once('_').unwrap();

        let hashed = HashedRawAccessToken::from(key);

        assert_eq!(hashed.prefix(), AccessTokenPrefix::PREFIX);
        assert_eq!(hashed.short_token(), expected_short);
        assert_eq!(
            hashed.long_token_hash().len(),
            64,
            "SHA-256 hex digest should be 64 characters"
        );
    }

    #[test]
    fn should_produce_sha256_hash_when_converting_from_raw_token_for_oauth_secret() {
        let key = RawOAuthClientSecret::new();
        let key_str: String = key.clone().into();
        let stripped = key_str
            .strip_prefix("aurahistoria_oauth_client_secret_")
            .unwrap();
        let (expected_short, _) = stripped.split_once('_').unwrap();

        let hashed = HashedRawOAuthClientSecret::from(key);

        assert_eq!(hashed.prefix(), OAuthClientSecretPrefix::PREFIX);
        assert_eq!(hashed.short_token(), expected_short);
        assert_eq!(
            hashed.long_token_hash().len(),
            64,
            "SHA-256 hex digest should be 64 characters"
        );
    }

    // ── Equal / different hashes for access token ─────────────────────────

    #[test]
    fn should_produce_equal_hashes_when_converting_same_key_twice_for_access_token() {
        let key = RawAccessToken::new();
        let hash1 = HashedRawAccessToken::from(key.clone());
        let hash2 = HashedRawAccessToken::from(key);
        assert_eq!(hash1, hash2);
    }

    #[test]
    fn should_produce_different_hashes_when_converting_different_keys_for_access_token() {
        let key1 = RawAccessToken::new();
        let key2 = RawAccessToken::new();
        let hash1 = HashedRawAccessToken::from(key1);
        let hash2 = HashedRawAccessToken::from(key2);
        assert_ne!(hash1, hash2);
    }

    // ── From<PrefixedApiKey> for HashedRawToken ────────────────────────────

    #[test]
    fn should_produce_same_result_when_converting_from_prefixed_api_key_as_from_raw_token_for_access_token()
     {
        let key = RawAccessToken::new();
        let prefixed: PrefixedApiKey = (&key).into();

        let hash_from_key = HashedRawAccessToken::from(key);
        let hash_from_prefixed = HashedRawAccessToken::from(prefixed);

        assert_eq!(hash_from_key, hash_from_prefixed);
    }

    #[test]
    fn should_set_correct_prefix_when_converting_from_prefixed_api_key_for_access_token() {
        let key = RawAccessToken::new();
        let prefixed: PrefixedApiKey = (&key).into();
        let hashed = HashedRawAccessToken::from(prefixed);
        assert_eq!(hashed.prefix(), AccessTokenPrefix::PREFIX);
    }

    // ── Error messages ────────────────────────────────────────────────────

    #[test]
    fn should_include_input_value_in_error_message_when_parsing_fails_for_access_token() {
        let bad_key = "totally_wrong_format";
        let err = RawAccessToken::try_from(bad_key.to_string()).unwrap_err();
        assert!(err.to_string().contains(bad_key));
    }

    #[test]
    fn should_include_input_value_in_error_message_when_parsing_fails_for_oauth_secret() {
        let bad_key = "totally_wrong_format";
        let err = RawOAuthClientSecret::try_from(bad_key.to_string()).unwrap_err();
        assert!(err.to_string().contains(bad_key));
    }

    #[rstest]
    #[case::wrong_prefix("wrongprefix_12345678901_123456789012345678901234567890123")]
    #[case::no_separator("aurahistoria_accesstoken_12345678901123456789012345678901234567890123")]
    #[trace]
    fn should_include_bad_key_in_error_message_for_each_validation_branch_for_access_token(
        #[case] bad_key: &str,
    ) {
        let err = RawAccessToken::try_from(bad_key.to_string()).unwrap_err();
        assert!(err.to_string().contains(bad_key));
    }

    #[rstest]
    #[case::wrong_prefix("wrongprefix_12345678901_123456789012345678901234567890123")]
    #[case::no_separator(
        "aurahistoria_oauth_client_secret_12345678901123456789012345678901234567890123"
    )]
    #[trace]
    fn should_include_bad_key_in_error_message_for_each_validation_branch_for_oauth_secret(
        #[case] bad_key: &str,
    ) {
        let err = RawOAuthClientSecret::try_from(bad_key.to_string()).unwrap_err();
        assert!(err.to_string().contains(bad_key));
    }
}

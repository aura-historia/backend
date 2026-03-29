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

    if short_token.len() != 8 {
        return Err(InvalidPartnerShopApiKeyError(
            api_key.to_string(),
            "the short token should be 8 characters ".to_string(),
        ));
    }
    if long_token.len() != 32 {
        return Err(InvalidPartnerShopApiKeyError(
            api_key.to_string(),
            "the long token should be 32 characters ".to_string(),
        ));
    }

    Ok((short_token.to_string(), long_token.to_string()))
}

#[derive(Debug, Clone, PartialEq)]
pub struct HashedPartnerShopApiKey {
    prefix: String,
    short_token: String,
    long_token_hash: String,
}

impl From<PartnerShopApiKey> for HashedPartnerShopApiKey {
    fn from(value: PartnerShopApiKey) -> Self {
        PrefixedApiKey::from_string(&value.0)
            .expect("shouldn't fail parsing ParnterShopKey as PrefixedApiKey by construction")
            .into()
    }
}

impl HashedPartnerShopApiKey {
    pub fn new(prefix: String, short_token: String, long_token_hash: String) -> Self {
        Self {
            prefix,
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

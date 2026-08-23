use application::error::BoxError;
use oauth_core::third_party_exchange_code::{ThirdPartyExchangeCode, ThirdPartyExchangeCodeGrant};

#[derive(Debug, thiserror::Error)]
pub enum OAuthCodeRepositoryError {
    #[error("oauth code already exists")]
    Conflict {
        #[source]
        source: BoxError,
    },
    #[error("temporary oauth code repository failure")]
    TemporarilyUnavailable {
        #[source]
        source: BoxError,
    },
    #[error("invalid persisted oauth code state")]
    InvalidPersistedState {
        #[source]
        source: BoxError,
    },
    #[error("internal oauth code repository failure")]
    Internal {
        #[source]
        source: BoxError,
    },
}

#[async_trait::async_trait]
pub trait ThirdPartyExchangeCodeRepository: Send + Sync {
    async fn insert(
        &self,
        grant: ThirdPartyExchangeCodeGrant,
    ) -> Result<(), OAuthCodeRepositoryError>;
    async fn find_by_code(
        &self,
        code: &ThirdPartyExchangeCode,
    ) -> Result<Option<ThirdPartyExchangeCodeGrant>, OAuthCodeRepositoryError>;
    async fn delete(&self, code: &ThirdPartyExchangeCode) -> Result<(), OAuthCodeRepositoryError>;
}

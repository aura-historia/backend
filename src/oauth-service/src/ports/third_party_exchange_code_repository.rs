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
pub trait ThirdPartyExchangeCodeRepository: Send {
    async fn insert(
        &mut self,
        grant: ThirdPartyExchangeCodeGrant,
    ) -> Result<(), OAuthCodeRepositoryError>;

    async fn consume_by_code(
        &mut self,
        code: &ThirdPartyExchangeCode,
    ) -> Result<Option<ThirdPartyExchangeCodeGrant>, OAuthCodeRepositoryError>;
}

pub trait ThirdPartyExchangeCodeRepositoryFactory<Tx>: Send + Sync {
    fn in_transaction<'tx>(
        &'tx self,
        tx: &'tx mut Tx,
    ) -> impl ThirdPartyExchangeCodeRepository + 'tx;
}

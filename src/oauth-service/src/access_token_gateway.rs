use crate::ports::{
    IssuedAccessToken, NewOAuthAccessToken, OAuthAccessTokenGateway, OAuthAccessTokenGatewayError,
};

use time::OffsetDateTime;
use user_core::access_token::{
    AccessToken, AccessTokenId, AccessTokenName, HashedRawAccessToken, NewAccessToken,
    RawAccessToken,
};
use user_service::ports::{AccessTokenStore, AccessTokenStoreError};

#[derive(Clone)]
pub struct StoreOAuthAccessTokenGateway<S> {
    store: S,
}

impl<S> StoreOAuthAccessTokenGateway<S> {
    pub fn new(store: S) -> Self {
        Self { store }
    }
}

#[async_trait::async_trait]
impl<S> OAuthAccessTokenGateway for StoreOAuthAccessTokenGateway<S>
where
    S: AccessTokenStore,
{
    async fn issue(
        &self,
        token: NewOAuthAccessToken,
    ) -> Result<IssuedAccessToken, OAuthAccessTokenGatewayError> {
        let raw = RawAccessToken::new();
        let now = OffsetDateTime::now_utc();
        let access_token = AccessToken::create(NewAccessToken {
            id: AccessTokenId::new(),
            hashed_token: raw.clone().into(),
            user_id: token.user_id,
            name: AccessTokenName::from(
                format!("{} (OAuth-Client {})", token.client_name, token.client_id).as_str(),
            ),
            scopes: token.scopes,
            origin: user_core::access_token::AccessTokenOrigin::OAuth {
                client_id: token.client_id,
            },
            expires: None,
        });
        self.store.insert(access_token.clone()).await?;
        Ok(IssuedAccessToken {
            raw,
            expires: access_token.expires(),
            scopes: access_token.scopes().clone(),
            user_id: access_token.user_id(),
            origin: access_token.origin().clone(),
            issued_at: Some(now),
        })
    }

    async fn delete_raw(&self, raw: &RawAccessToken) -> Result<(), OAuthAccessTokenGatewayError> {
        let hashed = HashedRawAccessToken::from(raw.clone());
        let token = self
            .store
            .find_by_hashed_token(&hashed)
            .await?
            .ok_or(OAuthAccessTokenGatewayError::NotFound)?;
        self.store.delete(&token.user_id(), &token.id()).await?;
        Ok(())
    }

    async fn find_raw(
        &self,
        raw: &RawAccessToken,
    ) -> Result<IssuedAccessToken, OAuthAccessTokenGatewayError> {
        let hashed = HashedRawAccessToken::from(raw.clone());
        let token = self
            .store
            .find_by_hashed_token(&hashed)
            .await?
            .ok_or(OAuthAccessTokenGatewayError::NotFound)?;
        if token.is_expired_at(OffsetDateTime::now_utc()) {
            return Err(OAuthAccessTokenGatewayError::Expired);
        }
        Ok(IssuedAccessToken {
            raw: raw.clone(),
            expires: token.expires(),
            scopes: token.scopes().clone(),
            user_id: token.user_id(),
            origin: token.origin().clone(),
            issued_at: None,
        })
    }
}

impl From<AccessTokenStoreError> for OAuthAccessTokenGatewayError {
    fn from(error: AccessTokenStoreError) -> Self {
        match error {
            AccessTokenStoreError::Conflict { source } => Self::Internal { source },
            AccessTokenStoreError::TemporarilyUnavailable { source } => {
                Self::TemporarilyUnavailable { source }
            }
            AccessTokenStoreError::InvalidPersistedState { source } => {
                Self::InvalidPersistedState { source }
            }
            AccessTokenStoreError::Internal { source } => Self::Internal { source },
        }
    }
}

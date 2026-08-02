use crate::authorization_code_record::AuthorizationCodeRecord;
use crate::client_record::OAuthClientRecord;
use crate::client_record_update::OAuthClientRecordUpdate;
use crate::repository::{OAuthDynamoDbRepositoryImpl, OAuthRepository};
use crate::third_party_exchange_code_record::ThirdPartyExchangeCodeRecord;
use common::error::boxed::box_error;
use common::oauth_client_id::OAuthClientId;
use oauth_core::authorization_code::{AuthorizationCode, OAuthAuthorizationCode};
use oauth_core::client::OAuthClient;
use oauth_core::third_party_exchange_code::{ThirdPartyExchangeCode, ThirdPartyExchangeCodeGrant};
use oauth_service::ports::{
    AuthorizationCodeRepository, OAuthClientPatch, OAuthClientReader, OAuthClientRepository,
    OAuthClientRepositoryError, OAuthCodeRepositoryError, ThirdPartyExchangeCodeRepository,
};

fn client_error<E>(source: E) -> OAuthClientRepositoryError
where
    E: std::error::Error + Send + Sync + 'static,
{
    OAuthClientRepositoryError::Internal {
        source: box_error(source),
    }
}

fn code_error<E>(source: E) -> OAuthCodeRepositoryError
where
    E: std::error::Error + Send + Sync + 'static,
{
    OAuthCodeRepositoryError::Internal {
        source: box_error(source),
    }
}

#[async_trait::async_trait]
impl OAuthClientReader for OAuthDynamoDbRepositoryImpl<'_> {
    async fn find_by_id(
        &self,
        client_id: &OAuthClientId,
    ) -> Result<Option<OAuthClient>, OAuthClientRepositoryError> {
        self.get_client_record(client_id)
            .await
            .map(|item| item.map(Into::into))
            .map_err(client_error)
    }

    async fn list(&self) -> Result<Vec<OAuthClient>, OAuthClientRepositoryError> {
        self.query_client_records()
            .await
            .map(|items| items.into_iter().map(Into::into).collect())
            .map_err(client_error)
    }
}

#[async_trait::async_trait]
impl OAuthClientRepository for OAuthDynamoDbRepositoryImpl<'_> {
    async fn insert(
        &self,
        client: OAuthClient,
        raw_secret: user_core::access_token::RawOAuthClientSecret,
    ) -> Result<(), OAuthClientRepositoryError> {
        self.put_client_record(OAuthClientRecord::from((client, raw_secret)))
            .await
            .map(|_| ())
            .map_err(client_error)
    }

    async fn update(
        &self,
        client_id: &OAuthClientId,
        patch: OAuthClientPatch,
    ) -> Result<Option<OAuthClient>, OAuthClientRepositoryError> {
        let update = OAuthClientRecordUpdate {
            name: patch.name,
            redirect_uris: patch.redirect_uris,
            tos_uri: patch.tos_uri,
            policy_uri: patch.policy_uri,
            client_uri: patch.client_uri,
            logo_uri: patch.logo_uri,
            scopes: patch
                .scopes
                .map(|scopes| scopes.into_iter().map(Into::into).collect()),
            updated_by: patch.updated_by.into(),
            updated: patch.updated,
        };
        self.update_client_record(client_id, update)
            .await
            .map(|item| item.map(Into::into))
            .map_err(client_error)
    }

    async fn delete(&self, client_id: &OAuthClientId) -> Result<(), OAuthClientRepositoryError> {
        self.delete_client_record(client_id)
            .await
            .map(|_| ())
            .map_err(client_error)
    }
}

#[async_trait::async_trait]
impl AuthorizationCodeRepository for OAuthDynamoDbRepositoryImpl<'_> {
    async fn insert(&self, code: AuthorizationCode) -> Result<(), OAuthCodeRepositoryError> {
        self.put_authorization_code_record(AuthorizationCodeRecord::from(code))
            .await
            .map(|_| ())
            .map_err(code_error)
    }

    async fn find_by_code(
        &self,
        code: &OAuthAuthorizationCode,
    ) -> Result<Option<AuthorizationCode>, OAuthCodeRepositoryError> {
        self.get_authorization_code_record(code)
            .await
            .map(|item| item.map(Into::into))
            .map_err(code_error)
    }

    async fn delete(&self, code: &OAuthAuthorizationCode) -> Result<(), OAuthCodeRepositoryError> {
        self.delete_authorization_code_record(code)
            .await
            .map(|_| ())
            .map_err(code_error)
    }
}

#[async_trait::async_trait]
impl ThirdPartyExchangeCodeRepository for OAuthDynamoDbRepositoryImpl<'_> {
    async fn insert(
        &self,
        grant: ThirdPartyExchangeCodeGrant,
    ) -> Result<(), OAuthCodeRepositoryError> {
        self.put_third_party_exchange_code_record(ThirdPartyExchangeCodeRecord::from(grant))
            .await
            .map(|_| ())
            .map_err(code_error)
    }

    async fn find_by_code(
        &self,
        code: &ThirdPartyExchangeCode,
    ) -> Result<Option<ThirdPartyExchangeCodeGrant>, OAuthCodeRepositoryError> {
        self.get_third_party_exchange_code_record(code)
            .await
            .map(|item| item.map(Into::into))
            .map_err(code_error)
    }

    async fn delete(&self, code: &ThirdPartyExchangeCode) -> Result<(), OAuthCodeRepositoryError> {
        self.delete_third_party_exchange_code_record(code)
            .await
            .map(|_| ())
            .map_err(code_error)
    }
}

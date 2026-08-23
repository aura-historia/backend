use crate::authorization_code_record::{self, AuthorizationCodeRecord};
use crate::repository::OAuthDynamoDbStore;

use application::error::box_error;
use aws_sdk_dynamodb::types::AttributeValue;
use oauth_core::authorization_code::{AuthorizationCode, OAuthAuthorizationCode};
use oauth_service::ports::{AuthorizationCodeRepository, OAuthCodeRepositoryError};

fn code_error<E>(source: E) -> OAuthCodeRepositoryError
where
    E: std::error::Error + Send + Sync + 'static,
{
    OAuthCodeRepositoryError::Internal {
        source: box_error(source),
    }
}

#[async_trait::async_trait]
impl AuthorizationCodeRepository for OAuthDynamoDbStore<'_> {
    async fn insert(&self, code: AuthorizationCode) -> Result<(), OAuthCodeRepositoryError> {
        let payload =
            serde_dynamo::to_item(AuthorizationCodeRecord::from(code)).map_err(code_error)?;
        self.client()
            .put_item()
            .table_name(self.table())
            .set_item(Some(payload))
            .send()
            .await
            .map(|_| ())
            .map_err(code_error)
    }

    async fn find_by_code(
        &self,
        code: &OAuthAuthorizationCode,
    ) -> Result<Option<AuthorizationCode>, OAuthCodeRepositoryError> {
        let Some(item) = self
            .client()
            .get_item()
            .table_name(self.table())
            .key(
                "pk",
                AttributeValue::S(authorization_code_record::mk_pk(code)),
            )
            .key(
                "sk",
                AttributeValue::S(authorization_code_record::mk_sk().to_owned()),
            )
            .send()
            .await
            .map_err(code_error)?
            .item
        else {
            return Ok(None);
        };
        let record =
            serde_dynamo::from_item::<_, AuthorizationCodeRecord>(item).map_err(|source| {
                OAuthCodeRepositoryError::InvalidPersistedState {
                    source: box_error(source),
                }
            })?;
        Ok(Some(record.into()))
    }

    async fn delete(&self, code: &OAuthAuthorizationCode) -> Result<(), OAuthCodeRepositoryError> {
        self.client()
            .delete_item()
            .table_name(self.table())
            .key(
                "pk",
                AttributeValue::S(authorization_code_record::mk_pk(code)),
            )
            .key(
                "sk",
                AttributeValue::S(authorization_code_record::mk_sk().to_owned()),
            )
            .send()
            .await
            .map(|_| ())
            .map_err(code_error)
    }
}

use crate::repository::OAuthDynamoDbStore;
use crate::third_party_exchange_code_record::{self, ThirdPartyExchangeCodeRecord};

use application::error::box_error;
use aws_sdk_dynamodb::types::AttributeValue;
use oauth_core::third_party_exchange_code::{ThirdPartyExchangeCode, ThirdPartyExchangeCodeGrant};
use oauth_service::ports::{OAuthCodeRepositoryError, ThirdPartyExchangeCodeRepository};

fn code_error<E>(source: E) -> OAuthCodeRepositoryError
where
    E: std::error::Error + Send + Sync + 'static,
{
    OAuthCodeRepositoryError::Internal {
        source: box_error(source),
    }
}

#[async_trait::async_trait]
impl ThirdPartyExchangeCodeRepository for OAuthDynamoDbStore<'_> {
    async fn insert(
        &self,
        grant: ThirdPartyExchangeCodeGrant,
    ) -> Result<(), OAuthCodeRepositoryError> {
        let payload =
            serde_dynamo::to_item(ThirdPartyExchangeCodeRecord::from(grant)).map_err(code_error)?;
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
        code: &ThirdPartyExchangeCode,
    ) -> Result<Option<ThirdPartyExchangeCodeGrant>, OAuthCodeRepositoryError> {
        let Some(item) = self
            .client()
            .get_item()
            .table_name(self.table())
            .key(
                "pk",
                AttributeValue::S(third_party_exchange_code_record::mk_pk(code)),
            )
            .key(
                "sk",
                AttributeValue::S(third_party_exchange_code_record::mk_sk().to_owned()),
            )
            .send()
            .await
            .map_err(code_error)?
            .item
        else {
            return Ok(None);
        };
        let record =
            serde_dynamo::from_item::<_, ThirdPartyExchangeCodeRecord>(item).map_err(|source| {
                OAuthCodeRepositoryError::InvalidPersistedState {
                    source: box_error(source),
                }
            })?;
        Ok(Some(record.into()))
    }

    async fn delete(&self, code: &ThirdPartyExchangeCode) -> Result<(), OAuthCodeRepositoryError> {
        self.client()
            .delete_item()
            .table_name(self.table())
            .key(
                "pk",
                AttributeValue::S(third_party_exchange_code_record::mk_pk(code)),
            )
            .key(
                "sk",
                AttributeValue::S(third_party_exchange_code_record::mk_sk().to_owned()),
            )
            .send()
            .await
            .map(|_| ())
            .map_err(code_error)
    }
}

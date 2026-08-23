use crate::client_record::{self, OAuthClientRecord};
use crate::repository::OAuthDynamoDbStore;
use application::error::box_error;
use aws_sdk_dynamodb::types::AttributeValue;
use oauth_core::client::OAuthClient;
use oauth_service::ports::{OAuthClientReader, OAuthClientRepositoryError};

fn client_error<E>(source: E) -> OAuthClientRepositoryError
where
    E: std::error::Error + Send + Sync + 'static,
{
    OAuthClientRepositoryError::Internal {
        source: box_error(source),
    }
}

#[async_trait::async_trait]
impl OAuthClientReader for OAuthDynamoDbStore<'_> {
    async fn list(&self) -> Result<Vec<OAuthClient>, OAuthClientRepositoryError> {
        let items = self
            .client()
            .query()
            .table_name(self.table())
            .key_condition_expression("#pk = :pk_val AND begins_with(#sk, :sk_prefix)")
            .expression_attribute_names("#pk", "pk")
            .expression_attribute_names("#sk", "sk")
            .expression_attribute_values(
                ":pk_val",
                AttributeValue::S(client_record::mk_pk().to_owned()),
            )
            .expression_attribute_values(
                ":sk_prefix",
                AttributeValue::S("oauth_client#".to_owned()),
            )
            .send()
            .await
            .map_err(client_error)?
            .items
            .unwrap_or_default();

        items
            .into_iter()
            .map(|item| {
                serde_dynamo::from_item::<_, OAuthClientRecord>(item)
                    .map(Into::into)
                    .map_err(|source| OAuthClientRepositoryError::InvalidPersistedState {
                        source: box_error(source),
                    })
            })
            .collect()
    }
}

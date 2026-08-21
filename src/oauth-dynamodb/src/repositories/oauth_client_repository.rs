use crate::client_record::{self, OAuthClientRecord};
use crate::client_record_update::OAuthClientRecordUpdate;
use crate::repository::OAuthDynamoDbStore;

use crate::dynamodb_update::DynamoDbUpdate;
use application::error::box_error;
use aws_sdk_dynamodb::types::{AttributeValue, ReturnValue};
use credential_core::oauth_client_id::OAuthClientId;
use oauth_core::client::OAuthClient;
use oauth_service::ports::{OAuthClientPatch, OAuthClientRepository, OAuthClientRepositoryError};

fn client_error<E>(source: E) -> OAuthClientRepositoryError
where
    E: std::error::Error + Send + Sync + 'static,
{
    OAuthClientRepositoryError::Internal {
        source: box_error(source),
    }
}

#[async_trait::async_trait]
impl OAuthClientRepository for OAuthDynamoDbStore<'_> {
    async fn find_by_client_id(
        &self,
        client_id: &OAuthClientId,
    ) -> Result<Option<OAuthClient>, OAuthClientRepositoryError> {
        let Some(item) = self
            .client()
            .get_item()
            .table_name(self.table())
            .key("pk", AttributeValue::S(client_record::mk_pk().to_owned()))
            .key("sk", AttributeValue::S(client_record::mk_sk(client_id)))
            .send()
            .await
            .map_err(client_error)?
            .item
        else {
            return Ok(None);
        };
        let record = serde_dynamo::from_item::<_, OAuthClientRecord>(item).map_err(|source| {
            OAuthClientRepositoryError::InvalidPersistedState {
                source: box_error(source),
            }
        })?;
        Ok(Some(record.into()))
    }

    async fn insert(
        &self,
        client: OAuthClient,
        raw_secret: user_core::access_token::RawOAuthClientSecret,
    ) -> Result<(), OAuthClientRepositoryError> {
        let payload = serde_dynamo::to_item(OAuthClientRecord::from((client, raw_secret)))
            .map_err(client_error)?;
        self.client()
            .put_item()
            .table_name(self.table())
            .set_item(Some(payload))
            .send()
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
            updated: patch.updated,
        };
        let update_expr = update.into_update_expr().map_err(client_error)?;
        self.client()
            .update_item()
            .table_name(self.table())
            .key("pk", AttributeValue::S(client_record::mk_pk().to_owned()))
            .key("sk", AttributeValue::S(client_record::mk_sk(client_id)))
            .update_expression(update_expr.update_expr)
            .set_expression_attribute_names(Some(update_expr.expr_attr_names))
            .set_expression_attribute_values(Some(update_expr.expr_attr_values))
            .return_values(ReturnValue::AllNew)
            .send()
            .await
            .map_err(client_error)
            .and_then(|output| match output.attributes {
                Some(item) => serde_dynamo::from_item::<_, OAuthClientRecord>(item)
                    .map(OAuthClient::from)
                    .map(Some)
                    .map_err(|source| OAuthClientRepositoryError::InvalidPersistedState {
                        source: box_error(source),
                    }),
                None => Ok(None),
            })
    }

    async fn delete(&self, client_id: &OAuthClientId) -> Result<(), OAuthClientRepositoryError> {
        self.client()
            .delete_item()
            .table_name(self.table())
            .key("pk", AttributeValue::S(client_record::mk_pk().to_owned()))
            .key("sk", AttributeValue::S(client_record::mk_sk(client_id)))
            .send()
            .await
            .map(|_| ())
            .map_err(client_error)
    }
}

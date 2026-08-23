use crate::access_token_record::{self, AccessTokenRecord};
use application::error::box_error;
use aws_sdk_dynamodb::{Client, types::AttributeValue};
use time::OffsetDateTime;
use user_core::access_token::{AccessToken, AccessTokenId, HashedRawAccessToken};
use user_core::user_id::UserId;
use user_service::ports::{AccessTokenStore, AccessTokenStoreError};

const ACCESS_TOKEN_SK_PREFIX: &str = "access_token#";
const GSI1_NAME: &str = "gsi1";
const GSI1_USER_PREFIX: &str = "user#";

#[derive(Debug, Clone)]
pub struct DynamoDbAccessTokenStore<'a> {
    client: &'a Client,
    table: String,
}

impl<'a> DynamoDbAccessTokenStore<'a> {
    pub fn new(client: &'a Client, table: impl Into<String>) -> Self {
        Self {
            client,
            table: table.into(),
        }
    }

    fn map_deserialize_error(
        source: impl std::error::Error + Send + Sync + 'static,
    ) -> AccessTokenStoreError {
        AccessTokenStoreError::InvalidPersistedState {
            source: box_error(source),
        }
    }

    fn map_internal_error(
        source: impl std::error::Error + Send + Sync + 'static,
    ) -> AccessTokenStoreError {
        AccessTokenStoreError::Internal {
            source: box_error(source),
        }
    }

    fn map_read_error(
        source: impl std::error::Error + Send + Sync + 'static,
    ) -> AccessTokenStoreError {
        let message = source.to_string();
        if is_temporary_aws_error(&message) {
            AccessTokenStoreError::TemporarilyUnavailable {
                source: box_error(source),
            }
        } else {
            AccessTokenStoreError::Internal {
                source: box_error(source),
            }
        }
    }

    fn map_write_error(
        source: impl std::error::Error + Send + Sync + 'static,
    ) -> AccessTokenStoreError {
        let message = source.to_string();
        if is_conditional_check_error(&message) {
            AccessTokenStoreError::Conflict {
                source: box_error(source),
            }
        } else if is_temporary_aws_error(&message) {
            AccessTokenStoreError::TemporarilyUnavailable {
                source: box_error(source),
            }
        } else {
            AccessTokenStoreError::Internal {
                source: box_error(source),
            }
        }
    }

    fn decode_record(
        item: std::collections::HashMap<String, AttributeValue>,
    ) -> Result<AccessTokenRecord, AccessTokenStoreError> {
        serde_dynamo::from_item::<_, AccessTokenRecord>(item).map_err(|source| {
            tracing::error!(
                error = %source,
                r#type = %std::any::type_name::<AccessTokenRecord>(),
                "Failed deserializing AccessTokenRecord."
            );
            Self::map_deserialize_error(source)
        })
    }

    fn decode_token(record: AccessTokenRecord) -> Result<AccessToken, AccessTokenStoreError> {
        AccessToken::try_from(record).map_err(|source| {
            tracing::error!(
                error = %source,
                r#type = %std::any::type_name::<AccessToken>(),
                "Failed mapping AccessTokenRecord."
            );
            Self::map_deserialize_error(source)
        })
    }

    fn serialize_record(
        record: AccessTokenRecord,
    ) -> Result<std::collections::HashMap<String, AttributeValue>, AccessTokenStoreError> {
        serde_dynamo::to_item(record).map_err(Self::map_internal_error)
    }

    async fn find_existing_record(
        &self,
        user_id: &UserId,
        access_token_id: &AccessTokenId,
    ) -> Result<Option<AccessTokenRecord>, AccessTokenStoreError> {
        let item = self
            .client
            .get_item()
            .table_name(&self.table)
            .key("pk", AttributeValue::S(access_token_record::mk_pk(user_id)))
            .key(
                "sk",
                AttributeValue::S(access_token_record::mk_sk(access_token_id)),
            )
            .send()
            .await
            .map_err(Self::map_read_error)?
            .item;

        item.map(Self::decode_record).transpose()
    }
}

#[async_trait::async_trait]
impl AccessTokenStore for DynamoDbAccessTokenStore<'_> {
    async fn find_by_id(
        &self,
        user_id: &UserId,
        access_token_id: &AccessTokenId,
    ) -> Result<Option<AccessToken>, AccessTokenStoreError> {
        let item = self
            .client
            .get_item()
            .table_name(&self.table)
            .key("pk", AttributeValue::S(access_token_record::mk_pk(user_id)))
            .key(
                "sk",
                AttributeValue::S(access_token_record::mk_sk(access_token_id)),
            )
            .send()
            .await
            .map_err(Self::map_read_error)?
            .item;

        item.map(Self::decode_record)
            .transpose()?
            .map(Self::decode_token)
            .transpose()
    }

    async fn find_by_hashed_token(
        &self,
        hashed_token: &HashedRawAccessToken,
    ) -> Result<Option<AccessToken>, AccessTokenStoreError> {
        let items = self
            .client
            .query()
            .table_name(&self.table)
            .index_name(GSI1_NAME)
            .key_condition_expression(
                "#gsi1_pk = :gsi1_pk_val AND begins_with(#gsi1_sk, :gsi1_sk_prefix)",
            )
            .expression_attribute_names("#gsi1_pk", "gsi1_pk")
            .expression_attribute_names("#gsi1_sk", "gsi1_sk")
            .expression_attribute_values(
                ":gsi1_pk_val",
                AttributeValue::S(access_token_record::mk_gsi1_pk(hashed_token)),
            )
            .expression_attribute_values(
                ":gsi1_sk_prefix",
                AttributeValue::S(GSI1_USER_PREFIX.to_owned()),
            )
            .send()
            .await
            .map_err(Self::map_read_error)?
            .items
            .unwrap_or_default();

        for item in items {
            let record = Self::decode_record(item)?;
            if record.matches_hash(hashed_token) {
                return Self::decode_token(record).map(Some);
            }
        }

        Ok(None)
    }

    async fn list_for_user(
        &self,
        user_id: &UserId,
    ) -> Result<Vec<AccessToken>, AccessTokenStoreError> {
        let items = self
            .client
            .query()
            .table_name(&self.table)
            .key_condition_expression("#pk = :pk_val AND begins_with(#sk, :sk_prefix)")
            .expression_attribute_names("#pk", "pk")
            .expression_attribute_names("#sk", "sk")
            .expression_attribute_values(
                ":pk_val",
                AttributeValue::S(access_token_record::mk_pk(user_id)),
            )
            .expression_attribute_values(
                ":sk_prefix",
                AttributeValue::S(ACCESS_TOKEN_SK_PREFIX.to_owned()),
            )
            .send()
            .await
            .map_err(Self::map_read_error)?
            .items
            .unwrap_or_default();

        items
            .into_iter()
            .map(Self::decode_record)
            .map(|record| record.and_then(Self::decode_token))
            .collect()
    }

    async fn insert(&self, access_token: AccessToken) -> Result<(), AccessTokenStoreError> {
        let now = OffsetDateTime::now_utc();
        let item = Self::serialize_record(AccessTokenRecord::from_access_token(
            &access_token,
            now,
            now,
        ))?;
        self.client
            .put_item()
            .table_name(&self.table)
            .set_item(Some(item))
            .condition_expression("attribute_not_exists(pk) AND attribute_not_exists(sk)")
            .send()
            .await
            .map(|_| ())
            .map_err(Self::map_write_error)
    }

    async fn replace(&self, access_token: AccessToken) -> Result<(), AccessTokenStoreError> {
        let created = self
            .find_existing_record(&access_token.user_id(), &access_token.id())
            .await?
            .map(|record| record.created)
            .unwrap_or_else(OffsetDateTime::now_utc);
        let item = Self::serialize_record(AccessTokenRecord::from_access_token(
            &access_token,
            created,
            OffsetDateTime::now_utc(),
        ))?;
        self.client
            .put_item()
            .table_name(&self.table)
            .set_item(Some(item))
            .condition_expression("attribute_exists(pk) AND attribute_exists(sk)")
            .send()
            .await
            .map(|_| ())
            .map_err(Self::map_write_error)
    }

    async fn delete(
        &self,
        user_id: &UserId,
        access_token_id: &AccessTokenId,
    ) -> Result<(), AccessTokenStoreError> {
        self.client
            .delete_item()
            .table_name(&self.table)
            .key("pk", AttributeValue::S(access_token_record::mk_pk(user_id)))
            .key(
                "sk",
                AttributeValue::S(access_token_record::mk_sk(access_token_id)),
            )
            .send()
            .await
            .map(|_| ())
            .map_err(Self::map_write_error)
    }
}

fn is_conditional_check_error(message: &str) -> bool {
    message.contains("ConditionalCheckFailed")
}

fn is_temporary_aws_error(message: &str) -> bool {
    message.contains("Throttling")
        || message.contains("ProvisionedThroughputExceeded")
        || message.contains("RequestLimitExceeded")
        || message.contains("Timeout")
        || message.contains("timeout")
        || message.contains("dispatch failure")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_classify_conditional_check_error() {
        assert!(is_conditional_check_error(
            "ConditionalCheckFailedException"
        ));
        assert!(!is_conditional_check_error("ThrottlingException"));
    }

    #[test]
    fn should_classify_temporary_aws_errors() {
        for message in [
            "ThrottlingException",
            "ProvisionedThroughputExceededException",
            "RequestLimitExceeded",
            "Timeout error",
            "dispatch failure",
        ] {
            assert!(is_temporary_aws_error(message));
        }
        assert!(!is_temporary_aws_error("ValidationException"));
    }
}

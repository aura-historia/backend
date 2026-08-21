use super::product_details_reader::{ProductDetailsRow, product_details_select};
use application::error::box_error;
use product_core::product_id::ProductId;
use product_service::ports::{
    PersonalizedProductDetailsReadModel, ProductDetailsBatchReadError,
    ProductDetailsBatchReadRequest, ProductDetailsBatchReader,
};
use sqlx::PgPool;
use std::collections::HashMap;

#[derive(Debug, Clone)]
pub struct SqlxProductDetailsBatchReader {
    pool: PgPool,
}

impl SqlxProductDetailsBatchReader {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[derive(Debug, thiserror::Error)]
#[error("product details batch search-filter identifier conversion failed")]
struct ProductDetailsBatchSearchFilterIdError(#[source] uuid::Error);

#[derive(Debug, thiserror::Error)]
#[error("product details batch SQL query failed")]
struct ProductDetailsBatchQuerySqlxError(#[source] sqlx::Error);

#[derive(Debug, thiserror::Error)]
#[error("product details batch row could not map to the read model")]
struct ProductDetailsBatchReadModelMappingError;

impl From<ProductDetailsBatchSearchFilterIdError> for ProductDetailsBatchReadError {
    fn from(source: ProductDetailsBatchSearchFilterIdError) -> Self {
        Self::QueryFailed {
            source: box_error(source),
        }
    }
}

impl From<ProductDetailsBatchQuerySqlxError> for ProductDetailsBatchReadError {
    fn from(source: ProductDetailsBatchQuerySqlxError) -> Self {
        Self::QueryFailed {
            source: box_error(source),
        }
    }
}

impl From<ProductDetailsBatchReadModelMappingError> for ProductDetailsBatchReadError {
    fn from(source: ProductDetailsBatchReadModelMappingError) -> Self {
        Self::InvalidReadModel {
            source: box_error(source),
        }
    }
}

#[async_trait::async_trait]
impl ProductDetailsBatchReader for SqlxProductDetailsBatchReader {
    async fn find_for_user(
        &self,
        request: &ProductDetailsBatchReadRequest,
    ) -> Result<HashMap<ProductId, PersonalizedProductDetailsReadModel>, ProductDetailsBatchReadError>
    {
        if request.product_ids.is_empty() {
            return Ok(HashMap::new());
        }

        let product_ids = request
            .product_ids
            .iter()
            .copied()
            .map(uuid::Uuid::from)
            .collect::<Vec<_>>();
        let search_filter_id = uuid::Uuid::parse_str(&request.search_filter_id.to_string())
            .map_err(ProductDetailsBatchSearchFilterIdError)?;
        let select = product_details_select(
            r#"
    requested_products AS (
        SELECT DISTINCT requested.product_id
        FROM UNNEST($3::uuid[]) AS requested(product_id)
    ),
    notification_states AS (
        SELECT
            notification.product_id,
            array_agg(
                notification.notification_id
                ORDER BY notification.created DESC, notification.notification_id DESC
            ) AS unseen_notification_ids
        FROM notifications notification
        JOIN requested_products requested
            ON requested.product_id = notification.product_id
        WHERE notification.user_id = $2
            AND notification.seen = false
        GROUP BY notification.product_id
    )
"#,
        )
        .replace(
            "AND matched.product_id = p.product_id",
            "AND matched.product_id = p.product_id AND matched.user_search_filter_id = $4",
        );
        let rows = sqlx::query_as::<_, ProductDetailsRow>(&format!(
            "{select} WHERE p.product_id = ANY($3)"
        ))
        .bind(request.language.as_str())
        .bind(uuid::Uuid::from(request.user_id))
        .bind(product_ids)
        .bind(search_filter_id)
        .fetch_all(&self.pool)
        .await
        .map_err(ProductDetailsBatchQuerySqlxError)?;

        rows.into_iter()
            .map(|row| {
                let product_id = ProductId::from(row.product_id);
                let details = PersonalizedProductDetailsReadModel::try_from(row)
                    .map_err(|_| ProductDetailsBatchReadModelMappingError)?;
                Ok((product_id, details))
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_preserve_sqlx_query_source() {
        let error: ProductDetailsBatchReadError =
            ProductDetailsBatchQuerySqlxError(sqlx::Error::RowNotFound).into();

        let ProductDetailsBatchReadError::QueryFailed { source } = error else {
            panic!("expected batch query failure");
        };
        assert!(
            source
                .downcast_ref::<ProductDetailsBatchQuerySqlxError>()
                .is_some()
        );
        assert!(source.source().is_some());
    }

    #[test]
    fn should_map_read_model_failure_without_exposing_row() {
        let error: ProductDetailsBatchReadError = ProductDetailsBatchReadModelMappingError.into();

        let ProductDetailsBatchReadError::InvalidReadModel { source } = error else {
            panic!("expected invalid batch read model");
        };
        assert!(
            source
                .downcast_ref::<ProductDetailsBatchReadModelMappingError>()
                .is_some()
        );
    }
}

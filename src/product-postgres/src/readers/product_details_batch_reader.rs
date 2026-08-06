use super::product_details_reader::{ProductDetailsRow, SELECT_PRODUCT_DETAILS};
use common::product_id::ProductId;
use product_service::ports::{
    ProductDetailsBatchReadError, ProductDetailsBatchReadRequest, ProductDetailsBatchReader,
};
use product_service::use_cases::PersonalizedProductDetailsView;
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

#[async_trait::async_trait]
impl ProductDetailsBatchReader for SqlxProductDetailsBatchReader {
    async fn find_for_user(
        &self,
        request: &ProductDetailsBatchReadRequest,
    ) -> Result<HashMap<ProductId, PersonalizedProductDetailsView>, ProductDetailsBatchReadError>
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
            .map_err(|_| ProductDetailsBatchReadError::QueryFailed)?;
        let select = SELECT_PRODUCT_DETAILS.replace(
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
        .map_err(|_| ProductDetailsBatchReadError::QueryFailed)?;

        rows.into_iter()
            .map(|row| {
                let product_id = ProductId::from(row.product_id);
                let details = PersonalizedProductDetailsView::try_from(row)
                    .map_err(|_| ProductDetailsBatchReadError::InvalidReadModel)?;
                Ok((product_id, details))
            })
            .collect()
    }
}

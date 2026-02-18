use crate::adapter::ClassifyCategoryAdapter;
use common::{batch::Batch, event_id::EventId};
use product::core::{
    product::Product,
    product_event::{
        ProductEvent, ProductEventPayload,
        enrichment::{
            ClassifiedCategoryProductEnrichmentEventPayload, ProductEnrichmentEventPayload,
        },
    },
};
use product_classification::category::service::CategoryService;
use product_pipeline_common::process::{PipeProcessor, ProcessResult};
use std::{collections::HashSet, sync::Arc};
use time::OffsetDateTime;
use tracing::{error, warn};

pub struct ClassifyCategoryPipeProcesserImpl {
    classify_category_delegate: Arc<dyn ClassifyCategoryAdapter + Send + Sync>,
    category_service: Arc<dyn CategoryService + Send + Sync>,
}

impl ClassifyCategoryPipeProcesserImpl {
    pub fn new(
        classify_category_delegate: Arc<dyn ClassifyCategoryAdapter + Send + Sync>,
        category_service: Arc<dyn CategoryService + Send + Sync>,
    ) -> Self {
        Self {
            classify_category_delegate,
            category_service,
        }
    }
}

#[async_trait::async_trait]
impl PipeProcessor for ClassifyCategoryPipeProcesserImpl {
    async fn process(&self, products: Vec<Product>) -> ProcessResult {
        let mut successes = Vec::with_capacity(products.len());
        let mut failures = HashSet::new();

        // Find candidates
        let mut tie_breaker_inputs = Vec::with_capacity(products.len());
        for product in products {
            if let Some(ref text_embedding) = product.text_embedding {
                let similar_res = self.category_service.find_similar(text_embedding, 5).await;
                match similar_res {
                    Ok(categories) => {
                        if categories.is_empty() {
                            error!(
                                productId = %product.product_id,
                                shopId = %product.shop_id,
                                shopsProductId = %product.shops_product_id,
                                "No similar categories found for product.",
                            );
                            failures.insert(product.product_id);
                        } else {
                            let category_ids: Vec<String> = categories
                                .iter()
                                .map(|(c, _)| c.category_id.to_string())
                                .collect();
                            tie_breaker_inputs.push((product, category_ids));
                        }
                    }
                    Err(err) => {
                        error!(
                            productId = %product.product_id,
                            shopId = %product.shop_id,
                            shopsProductId = %product.shops_product_id,
                            error = ?err,
                            "Failed finding similar categories for product.",
                        );
                        failures.insert(product.product_id);
                    }
                }
            } else {
                error!(
                    productId = %product.product_id,
                    shopId = %product.shop_id,
                    shopsProductId = %product.shops_product_id,
                    "Product does not have text embedding.",
                );
                failures.insert(product.product_id);
            }
        }

        // Choose candidate
        for batch in Batch::chunked_from(tie_breaker_inputs.into_iter()) {
            let batch: Batch<_, 64> = batch;
            let in_iter = batch.iter().map(|(product, category_ids)| {
                (
                    product.native_title.payload.to_string(),
                    category_ids.clone(),
                )
            });
            let in_batch = Batch::try_from_iter(in_iter)
                .expect("shouldn't fail re-collecting batch of same size");

            let batch_res = self.classify_category_delegate.classify_category(&in_batch);
            match batch_res {
                Ok(batch_categories) => {
                    let local_successes = batch.into_iter().zip(batch_categories).filter_map(
                        |((product, candidates), chosen)| {
                            if candidates.contains(&chosen) {
                                let event = ProductEvent {
                                    aggregate_id: product.product_id,
                                    event_id: EventId::new(),
                                    timestamp: OffsetDateTime::now_utc(),
                                    payload: ProductEventPayload::ProductEnrichmentEvent(
                                        ProductEnrichmentEventPayload::ClassifiedCategory(
                                            ClassifiedCategoryProductEnrichmentEventPayload {
                                                shop_id: product.shop_id,
                                                shops_product_id: product.shops_product_id,
                                                category_id: chosen.into(),
                                            },
                                        ),
                                    ),
                                };
                                Some(event)
                            } else {
                                warn!(candidates = ?candidates, chosen = chosen, "Tie-Breaker responded with non-candidate category-id.");
                                failures.insert(product.product_id);
                                None
                            }
                        },
                    );
                    successes.extend(local_successes);
                }
                Err(err) => {
                    let local_failures =
                        batch.iter().map(|(p, _)| p.product_id).collect::<Vec<_>>();
                    error!(
                        productIds = ?local_failures,
                        error = ?err,
                        "Failed classifying categories for batch of products.",
                    );
                    failures.extend(local_failures.into_iter());
                }
            }
        }

        ProcessResult {
            successes,
            failures,
        }
    }
}

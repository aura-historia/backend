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

pub struct ClassifyCategoryPipeProcesserImpl<'a> {
    classify_category_delegate: Arc<dyn ClassifyCategoryAdapter + Send + Sync>,
    category_service: &'a (dyn CategoryService + Send + Sync),
}

impl<'a> ClassifyCategoryPipeProcesserImpl<'a> {
    pub fn new(
        classify_category_delegate: Arc<dyn ClassifyCategoryAdapter + Send + Sync>,
        category_service: &'a (dyn CategoryService + Send + Sync),
    ) -> Self {
        Self {
            classify_category_delegate,
            category_service,
        }
    }
}

#[async_trait::async_trait]
impl<'a> PipeProcessor for ClassifyCategoryPipeProcesserImpl<'a> {
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

#[cfg(test)]
mod tests {
    use super::ClassifyCategoryPipeProcesserImpl;
    use crate::adapter::MockClassifyCategoryAdapter;
    use common::{
        batch::Batch,
        category_key::{CategoryId, CategoryKey},
        language::domain::Language,
    };
    use fake::{Fake, Faker};
    use product::core::product::Product;
    use product_classification::category::core::Category;
    use product_classification::category::service::MockCategoryService;
    use product_pipeline_common::process::PipeProcessor;
    use pyo3::{PyErr, exceptions::PyTypeError};
    use rstest;
    use serde::de::Error;
    use std::collections::HashMap;
    use std::sync::Arc;
    use time::OffsetDateTime;

    fn mk_category(id: &str) -> Category {
        let mut display_name = HashMap::new();
        display_name.insert(Language::En, "category-name".into());
        let mut display_description = HashMap::new();
        display_description.insert(Language::En, "category-description".into());
        Category {
            category_id: CategoryId::from(id),
            category_key: CategoryKey::from(format!("{id}-key")),
            meta_name: "meta-name".into(),
            meta_description: "meta-description".into(),
            meta_keywords: vec!["meta-keyword".into()],
            embedding: vec![0.1; 4],
            display_name,
            display_description,
            created: OffsetDateTime::now_utc(),
            updated: OffsetDateTime::now_utc(),
        }
    }

    fn mk_product_with_embedding() -> Product {
        let mut product: Product = Faker.fake();
        product.text_embedding = Some(vec![0.1; 4]);
        product.native_title.payload = "Test product".into();
        product
    }

    #[tokio::test]
    async fn should_classify_category_for_product_with_embedding() {
        let category = mk_category("furniture");

        let mut category_service = MockCategoryService::default();
        category_service
            .expect_find_similar()
            .returning(move |_, _| {
                let category = category.clone();
                Box::pin(async move { Ok(vec![(category, 0.9)]) })
            });

        let mut adapter = MockClassifyCategoryAdapter::default();
        adapter.expect_classify_category().returning(|batch| {
            let chosen = batch
                .iter()
                .map(|(_, candidates)| candidates[0].clone())
                .collect::<Vec<_>>();
            Ok(Batch::try_from(chosen).unwrap())
        });

        let processor =
            ClassifyCategoryPipeProcesserImpl::new(Arc::new(adapter), &category_service);

        let product = mk_product_with_embedding();
        let product_id = product.product_id;
        let shop_id = product.shop_id;
        let shops_product_id = product.shops_product_id.clone();

        let actual = processor.process(vec![product]).await;

        assert!(actual.failures.is_empty());
        assert_eq!(1, actual.successes.len());

        let event = &actual.successes[0];
        assert_eq!(event.aggregate_id, product_id);

        let payload = event
            .payload
            .as_enrichment_event()
            .unwrap()
            .as_classified_category()
            .unwrap();
        assert_eq!(payload.shop_id, shop_id);
        assert_eq!(payload.shops_product_id, shops_product_id);
        assert_eq!(payload.category_id, CategoryId::from("furniture"));
    }

    #[tokio::test]
    async fn should_fail_when_product_has_no_text_embedding() {
        let mut adapter = MockClassifyCategoryAdapter::default();
        adapter.expect_classify_category().never();

        let mut category_service = MockCategoryService::default();
        category_service.expect_find_similar().never();

        let processor =
            ClassifyCategoryPipeProcesserImpl::new(Arc::new(adapter), &category_service);

        let mut product: Product = Faker.fake();
        product.text_embedding = None;

        let product_id = product.product_id;
        let actual = processor.process(vec![product]).await;

        assert!(actual.successes.is_empty());
        assert!(actual.failures.contains(&product_id));
    }

    #[tokio::test]
    async fn should_fail_when_find_similar_returns_error() {
        let mut adapter = MockClassifyCategoryAdapter::default();
        adapter.expect_classify_category().never();

        let mut category_service = MockCategoryService::default();
        category_service.expect_find_similar().returning(|_, _| {
            Box::pin(async move {
                Err(opensearch::Error::from(serde_json::Error::custom(
                    "Something went wrong",
                )))
            })
        });

        let processor =
            ClassifyCategoryPipeProcesserImpl::new(Arc::new(adapter), &category_service);

        let product = mk_product_with_embedding();
        let product_id = product.product_id;

        let actual = processor.process(vec![product]).await;

        assert!(actual.successes.is_empty());
        assert!(actual.failures.contains(&product_id));
    }

    #[tokio::test]
    async fn should_fail_when_find_similar_returns_empty() {
        let mut adapter = MockClassifyCategoryAdapter::default();
        adapter.expect_classify_category().never();

        let mut category_service = MockCategoryService::default();
        category_service
            .expect_find_similar()
            .returning(|_, _| Box::pin(async move { Ok(vec![]) }));

        let processor =
            ClassifyCategoryPipeProcesserImpl::new(Arc::new(adapter), &category_service);

        let product = mk_product_with_embedding();
        let product_id = product.product_id;

        let actual = processor.process(vec![product]).await;

        assert!(actual.successes.is_empty());
        assert!(actual.failures.contains(&product_id));
    }

    #[tokio::test]
    async fn should_fail_when_classify_category_returns_non_candidate() {
        let category = mk_category("furniture");

        let mut category_service = MockCategoryService::default();
        category_service
            .expect_find_similar()
            .returning(move |_, _| {
                let category = category.clone();
                Box::pin(async move { Ok(vec![(category, 0.9)]) })
            });

        let mut adapter = MockClassifyCategoryAdapter::default();
        adapter
            .expect_classify_category()
            .returning(|_| Ok(Batch::try_from(vec!["decorative-objects".to_string()]).unwrap()));

        let processor =
            ClassifyCategoryPipeProcesserImpl::new(Arc::new(adapter), &category_service);

        let product = mk_product_with_embedding();
        let product_id = product.product_id;

        let actual = processor.process(vec![product]).await;

        assert!(actual.successes.is_empty());
        assert!(actual.failures.contains(&product_id));
    }

    #[tokio::test]
    async fn should_fail_batch_when_classify_category_adapter_errors() {
        let category = mk_category("furniture");

        let mut category_service = MockCategoryService::default();
        category_service
            .expect_find_similar()
            .returning(move |_, _| {
                let category = category.clone();
                Box::pin(async move { Ok(vec![(category, 0.9)]) })
            });

        let mut adapter = MockClassifyCategoryAdapter::default();
        adapter
            .expect_classify_category()
            .returning(|_| Err(PyErr::new::<PyTypeError, _>("Something went wrong")));

        let processor =
            ClassifyCategoryPipeProcesserImpl::new(Arc::new(adapter), &category_service);

        let products = vec![mk_product_with_embedding(), mk_product_with_embedding()];
        let product_ids = products.iter().map(|p| p.product_id).collect::<Vec<_>>();

        let actual = processor.process(products).await;

        assert!(actual.successes.is_empty());
        for product_id in product_ids {
            assert!(actual.failures.contains(&product_id));
        }
    }

    #[tokio::test]
    async fn should_partially_fail_when_some_products_lack_embedding() {
        let category = mk_category("furniture");

        let mut category_service = MockCategoryService::default();
        category_service
            .expect_find_similar()
            .times(2)
            .returning(move |_, _| {
                let category = category.clone();
                Box::pin(async move { Ok(vec![(category, 0.9)]) })
            });

        let mut adapter = MockClassifyCategoryAdapter::default();
        adapter.expect_classify_category().returning(|batch| {
            let chosen = batch
                .iter()
                .map(|(_, candidates)| candidates[0].clone())
                .collect::<Vec<_>>();
            Ok(Batch::try_from(chosen).unwrap())
        });

        let processor =
            ClassifyCategoryPipeProcesserImpl::new(Arc::new(adapter), &category_service);

        let mut products = vec![mk_product_with_embedding(), mk_product_with_embedding()];
        let mut missing = mk_product_with_embedding();
        missing.text_embedding = None;
        let missing_id = missing.product_id;
        products.push(missing);

        let actual = processor.process(products).await;

        assert_eq!(2, actual.successes.len());
        assert!(actual.failures.contains(&missing_id));
    }

    #[rstest::rstest]
    #[trace]
    #[case(0)]
    #[case(1)]
    #[case(7)]
    #[case(63)]
    #[case(64)]
    #[case(65)]
    #[case(128)]
    #[tokio::test]
    async fn should_process_classification(#[case] count: usize) {
        let category = mk_category("furniture");

        let mut category_service = MockCategoryService::default();
        category_service
            .expect_find_similar()
            .returning(move |_, _| {
                let category = category.clone();
                Box::pin(async move { Ok(vec![(category, 0.9)]) })
            });

        let mut adapter = MockClassifyCategoryAdapter::default();
        adapter.expect_classify_category().returning(|batch| {
            let chosen = batch
                .iter()
                .map(|(_, candidates)| candidates[0].clone())
                .collect::<Vec<_>>();
            Ok(Batch::try_from(chosen).unwrap())
        });

        let processor =
            ClassifyCategoryPipeProcesserImpl::new(Arc::new(adapter), &category_service);

        let products = fake::vec![Product; count]
            .into_iter()
            .map(|mut product| {
                product.text_embedding = Some(vec![0.1; 4]);
                product
            })
            .collect::<Vec<_>>();

        let actual = processor.process(products).await;

        assert!(actual.failures.is_empty());
        assert_eq!(count, actual.successes.len());
    }

    #[rstest::rstest]
    #[trace]
    #[case(0)]
    #[case(1)]
    #[case(63)]
    #[case(64)]
    #[case(65)]
    #[case(128)]
    #[case(129)]
    #[tokio::test]
    async fn should_partially_fail_when_classifier_fails_for_non_full_batches(
        #[case] count: usize,
    ) {
        let category = mk_category("furniture");

        let mut category_service = MockCategoryService::default();
        category_service
            .expect_find_similar()
            .returning(move |_, _| {
                let category = category.clone();
                Box::pin(async move { Ok(vec![(category, 0.9)]) })
            });

        let mut adapter = MockClassifyCategoryAdapter::default();
        adapter.expect_classify_category().returning(|batch| {
            if batch.len() == 64 {
                let chosen = batch
                    .iter()
                    .map(|(_, candidates)| candidates[0].clone())
                    .collect::<Vec<_>>();
                Ok(Batch::try_from(chosen).unwrap())
            } else {
                Err(PyErr::new::<PyTypeError, _>("Something went wrong"))
            }
        });

        let processor =
            ClassifyCategoryPipeProcesserImpl::new(Arc::new(adapter), &category_service);

        let products = fake::vec![Product; count]
            .into_iter()
            .map(|mut product| {
                product.text_embedding = Some(vec![0.1; 4]);
                product
            })
            .collect::<Vec<_>>();

        let actual = processor.process(products).await;

        assert_eq!(count % 64, actual.failures.len());
    }

    #[rstest::rstest]
    #[trace]
    #[case(0)]
    #[case(1)]
    #[case(2)]
    #[case(5)]
    #[case(10)]
    #[case(42)]
    #[case(64)]
    #[case(100)]
    #[tokio::test]
    async fn should_partially_fail_all_when_classifier_always_errors(#[case] count: usize) {
        let category = mk_category("furniture");

        let mut category_service = MockCategoryService::default();
        category_service
            .expect_find_similar()
            .returning(move |_, _| {
                let category = category.clone();
                Box::pin(async move { Ok(vec![(category, 0.9)]) })
            });

        let mut adapter = MockClassifyCategoryAdapter::default();
        adapter
            .expect_classify_category()
            .returning(|_| Err(PyErr::new::<PyTypeError, _>("Something went wrong")));

        let processor =
            ClassifyCategoryPipeProcesserImpl::new(Arc::new(adapter), &category_service);

        let products = fake::vec![Product; count]
            .into_iter()
            .map(|mut product| {
                product.text_embedding = Some(vec![0.1; 4]);
                product
            })
            .collect::<Vec<_>>();

        let actual = processor.process(products).await;

        assert!(actual.successes.is_empty());
        assert_eq!(count, actual.failures.len());
    }
}

use crate::adapter::ClassifyAdapter;
use common::{batch::Batch, event_id::EventId};
use product::core::{
    product::Product,
    product_event::{
        ProductEvent, ProductEventPayload,
        enrichment::{
            ClassifiedCategoryProductEnrichmentEventPayload,
            ClassifiedPeriodProductEnrichmentEventPayload, ProductEnrichmentEventPayload,
        },
    },
};
use product_classification::{category::service::CategoryService, period::service::PeriodService};
use product_pipeline_common::process::{PipeProcessor, ProcessResult};
use std::{collections::HashSet, sync::Arc};
use time::OffsetDateTime;
use tracing::{error, warn};

pub struct ClassifyPipeProcessorImpl<'a> {
    classify_delegate: Arc<dyn ClassifyAdapter + Send + Sync>,
    category_service: &'a (dyn CategoryService + Send + Sync),
    period_service: &'a (dyn PeriodService + Send + Sync),
}

impl<'a> ClassifyPipeProcessorImpl<'a> {
    pub fn new(
        classify_delegate: Arc<dyn ClassifyAdapter + Send + Sync>,
        category_service: &'a (dyn CategoryService + Send + Sync),
        period_service: &'a (dyn PeriodService + Send + Sync),
    ) -> Self {
        Self {
            classify_delegate,
            category_service,
            period_service,
        }
    }
}

#[async_trait::async_trait]
impl<'a> PipeProcessor for ClassifyPipeProcessorImpl<'a> {
    async fn process(&self, products: Vec<Product>) -> ProcessResult {
        let mut successes = Vec::with_capacity(products.len() * 2);
        let mut failures = HashSet::new();

        // Find candidates for both category and period
        let mut tie_breaker_inputs = Vec::with_capacity(products.len());
        for product in products {
            if let Some(ref text_embedding) = product.text_embedding {
                let (similar_categories_res, similar_periods_res) = tokio::join!(
                    self.category_service.find_similar(text_embedding, 5),
                    self.period_service.find_similar(text_embedding, 5),
                );

                let category_ids = match similar_categories_res {
                    Ok(categories) => {
                        if categories.is_empty() {
                            error!(
                                productId = %product.product_id,
                                shopId = %product.shop_id,
                                shopsProductId = %product.shops_product_id,
                                "No similar categories found for product.",
                            );
                            failures.insert(product.product_id);
                            continue;
                        }
                        categories
                            .iter()
                            .map(|(c, _)| c.category_id.to_string())
                            .collect::<Vec<_>>()
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
                        continue;
                    }
                };

                let period_ids = match similar_periods_res {
                    Ok(periods) => {
                        if periods.is_empty() {
                            error!(
                                productId = %product.product_id,
                                shopId = %product.shop_id,
                                shopsProductId = %product.shops_product_id,
                                "No similar periods found for product.",
                            );
                            failures.insert(product.product_id);
                            continue;
                        }
                        periods
                            .iter()
                            .map(|(p, _)| p.period_id.to_string())
                            .collect::<Vec<_>>()
                    }
                    Err(err) => {
                        error!(
                            productId = %product.product_id,
                            shopId = %product.shop_id,
                            shopsProductId = %product.shops_product_id,
                            error = ?err,
                            "Failed finding similar periods for product.",
                        );
                        failures.insert(product.product_id);
                        continue;
                    }
                };

                tie_breaker_inputs.push((product, category_ids, period_ids));
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

        // Sort by prompt length so each batch contains items of similar length,
        // reducing padding overhead in transformer models.
        tie_breaker_inputs.sort_by_key(|(product, category_ids, period_ids)| {
            product.native_title.payload.len()
                + category_ids.iter().map(|id| id.len()).sum::<usize>()
                + period_ids.iter().map(|id| id.len()).sum::<usize>()
        });

        // Choose candidates
        for batch in Batch::chunked_from(tie_breaker_inputs.into_iter()) {
            let batch: Batch<_, 64> = batch;
            let in_iter = batch.iter().map(|(product, category_ids, period_ids)| {
                (
                    product.native_title.payload.to_string(),
                    category_ids.clone(),
                    period_ids.clone(),
                )
            });
            let in_batch = Batch::try_from_iter(in_iter)
                .expect("shouldn't fail re-collecting batch of same size");

            let batch_res = self.classify_delegate.classify(&in_batch);
            match batch_res {
                Ok(batch_classifications) => {
                    let local_successes = batch
                        .into_iter()
                        .zip(batch_classifications)
                        .filter_map(
                            |(
                                (product, category_candidates, period_candidates),
                                (chosen_category, chosen_period),
                            )| {
                                let category_valid = category_candidates.contains(&chosen_category);
                                let period_valid = period_candidates.contains(&chosen_period);

                                if !category_valid {
                                    warn!(candidates = ?category_candidates, chosen = chosen_category, "Tie-Breaker responded with non-candidate category-id.");
                                }
                                if !period_valid {
                                    warn!(candidates = ?period_candidates, chosen = chosen_period, "Tie-Breaker responded with non-candidate period-id.");
                                }

                                if !category_valid || !period_valid {
                                    failures.insert(product.product_id);
                                    return None;
                                }

                                let now = OffsetDateTime::now_utc();
                                let category_event = ProductEvent {
                                    aggregate_id: product.product_id,
                                    event_id: EventId::new(),
                                    timestamp: now,
                                    payload: ProductEventPayload::ProductEnrichmentEvent(
                                        ProductEnrichmentEventPayload::ClassifiedCategory(
                                            ClassifiedCategoryProductEnrichmentEventPayload {
                                                shop_id: product.shop_id,
                                                shops_product_id: product
                                                    .shops_product_id
                                                    .clone(),
                                                category_id: chosen_category.into(),
                                            },
                                        ),
                                    ),
                                };
                                let period_event = ProductEvent {
                                    aggregate_id: product.product_id,
                                    event_id: EventId::new(),
                                    timestamp: now,
                                    payload: ProductEventPayload::ProductEnrichmentEvent(
                                        ProductEnrichmentEventPayload::ClassifiedPeriod(
                                            ClassifiedPeriodProductEnrichmentEventPayload {
                                                shop_id: product.shop_id,
                                                shops_product_id: product.shops_product_id,
                                                period_id: chosen_period.into(),
                                            },
                                        ),
                                    ),
                                };
                                Some(vec![category_event, period_event])
                            },
                        )
                        .flatten();
                    successes.extend(local_successes);
                }
                Err(err) => {
                    let local_failures = batch
                        .iter()
                        .map(|(p, _, _)| p.product_id)
                        .collect::<Vec<_>>();
                    error!(
                        productIds = ?local_failures,
                        error = ?err,
                        "Failed classifying products for batch.",
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
    use super::ClassifyPipeProcessorImpl;
    use crate::adapter::MockClassifyAdapter;
    use common::{
        batch::Batch,
        category_key::{CategoryId, CategoryKey},
        language::domain::Language,
        period_key::{PeriodId, PeriodKey},
    };
    use fake::{Fake, Faker};
    use product::core::product::Product;
    use product_classification::category::core::Category;
    use product_classification::category::service::MockCategoryService;
    use product_classification::period::core::Period;
    use product_classification::period::service::MockPeriodService;
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

    fn mk_period(id: &str) -> Period {
        let mut display_name = HashMap::new();
        display_name.insert(Language::En, "period-name".into());
        let mut display_description = HashMap::new();
        display_description.insert(Language::En, "period-description".into());
        Period {
            period_id: PeriodId::from(id),
            period_key: PeriodKey::from(format!("{id}-key")),
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

    fn mk_adapter_returning_first_candidates() -> MockClassifyAdapter {
        let mut adapter = MockClassifyAdapter::default();
        adapter.expect_classify().returning(|batch| {
            let chosen = batch
                .iter()
                .map(|(_, categories, periods)| (categories[0].clone(), periods[0].clone()))
                .collect::<Vec<_>>();
            Ok(Batch::try_from(chosen).unwrap())
        });
        adapter
    }

    #[tokio::test]
    async fn should_classify_category_and_period_for_product_with_embedding() {
        let category = mk_category("furniture");
        let period = mk_period("baroque");

        let mut category_service = MockCategoryService::default();
        category_service
            .expect_find_similar()
            .returning(move |_, _| {
                let category = category.clone();
                Box::pin(async move { Ok(vec![(category, 0.9)]) })
            });

        let mut period_service = MockPeriodService::default();
        period_service.expect_find_similar().returning(move |_, _| {
            let period = period.clone();
            Box::pin(async move { Ok(vec![(period, 0.9)]) })
        });

        let adapter = mk_adapter_returning_first_candidates();

        let processor =
            ClassifyPipeProcessorImpl::new(Arc::new(adapter), &category_service, &period_service);

        let product = mk_product_with_embedding();
        let product_id = product.product_id;
        let shop_id = product.shop_id;
        let shops_product_id = product.shops_product_id.clone();

        let actual = processor.process(vec![product]).await;

        assert!(actual.failures.is_empty());
        assert_eq!(2, actual.successes.len());

        let category_payload = actual.successes[0]
            .payload
            .as_enrichment_event()
            .unwrap()
            .as_classified_category()
            .unwrap();
        assert_eq!(category_payload.shop_id, shop_id);
        assert_eq!(category_payload.shops_product_id, shops_product_id);
        assert_eq!(category_payload.category_id, CategoryId::from("furniture"));

        let period_payload = actual.successes[1]
            .payload
            .as_enrichment_event()
            .unwrap()
            .as_classified_period()
            .unwrap();
        assert_eq!(period_payload.shop_id, shop_id);
        assert_eq!(period_payload.shops_product_id, shops_product_id);
        assert_eq!(period_payload.period_id, PeriodId::from("baroque"));

        assert_eq!(actual.successes[0].aggregate_id, product_id);
        assert_eq!(actual.successes[1].aggregate_id, product_id);
    }

    #[tokio::test]
    async fn should_fail_when_product_has_no_text_embedding() {
        let mut adapter = MockClassifyAdapter::default();
        adapter.expect_classify().never();

        let mut category_service = MockCategoryService::default();
        category_service.expect_find_similar().never();

        let mut period_service = MockPeriodService::default();
        period_service.expect_find_similar().never();

        let processor =
            ClassifyPipeProcessorImpl::new(Arc::new(adapter), &category_service, &period_service);

        let mut product: Product = Faker.fake();
        product.text_embedding = None;

        let product_id = product.product_id;
        let actual = processor.process(vec![product]).await;

        assert!(actual.successes.is_empty());
        assert!(actual.failures.contains(&product_id));
    }

    #[tokio::test]
    async fn should_fail_when_find_similar_categories_returns_error() {
        let period = mk_period("baroque");
        let mut adapter = MockClassifyAdapter::default();
        adapter.expect_classify().never();

        let mut category_service = MockCategoryService::default();
        category_service.expect_find_similar().returning(|_, _| {
            Box::pin(async move {
                Err(opensearch::Error::from(serde_json::Error::custom(
                    "Something went wrong",
                )))
            })
        });

        let mut period_service = MockPeriodService::default();
        period_service.expect_find_similar().returning(move |_, _| {
            let period = period.clone();
            Box::pin(async move { Ok(vec![(period, 0.9)]) })
        });

        let processor =
            ClassifyPipeProcessorImpl::new(Arc::new(adapter), &category_service, &period_service);

        let product = mk_product_with_embedding();
        let product_id = product.product_id;

        let actual = processor.process(vec![product]).await;

        assert!(actual.successes.is_empty());
        assert!(actual.failures.contains(&product_id));
    }

    #[tokio::test]
    async fn should_fail_when_find_similar_periods_returns_error() {
        let category = mk_category("furniture");
        let mut adapter = MockClassifyAdapter::default();
        adapter.expect_classify().never();

        let mut category_service = MockCategoryService::default();
        category_service
            .expect_find_similar()
            .returning(move |_, _| {
                let category = category.clone();
                Box::pin(async move { Ok(vec![(category, 0.9)]) })
            });

        let mut period_service = MockPeriodService::default();
        period_service.expect_find_similar().returning(|_, _| {
            Box::pin(async move {
                Err(opensearch::Error::from(serde_json::Error::custom(
                    "Something went wrong",
                )))
            })
        });

        let processor =
            ClassifyPipeProcessorImpl::new(Arc::new(adapter), &category_service, &period_service);

        let product = mk_product_with_embedding();
        let product_id = product.product_id;

        let actual = processor.process(vec![product]).await;

        assert!(actual.successes.is_empty());
        assert!(actual.failures.contains(&product_id));
    }

    #[tokio::test]
    async fn should_fail_when_find_similar_categories_returns_empty() {
        let period = mk_period("baroque");
        let mut adapter = MockClassifyAdapter::default();
        adapter.expect_classify().never();

        let mut category_service = MockCategoryService::default();
        category_service
            .expect_find_similar()
            .returning(|_, _| Box::pin(async move { Ok(vec![]) }));

        let mut period_service = MockPeriodService::default();
        period_service.expect_find_similar().returning(move |_, _| {
            let period = period.clone();
            Box::pin(async move { Ok(vec![(period, 0.9)]) })
        });

        let processor =
            ClassifyPipeProcessorImpl::new(Arc::new(adapter), &category_service, &period_service);

        let product = mk_product_with_embedding();
        let product_id = product.product_id;

        let actual = processor.process(vec![product]).await;

        assert!(actual.successes.is_empty());
        assert!(actual.failures.contains(&product_id));
    }

    #[tokio::test]
    async fn should_fail_when_find_similar_periods_returns_empty() {
        let category = mk_category("furniture");
        let mut adapter = MockClassifyAdapter::default();
        adapter.expect_classify().never();

        let mut category_service = MockCategoryService::default();
        category_service
            .expect_find_similar()
            .returning(move |_, _| {
                let category = category.clone();
                Box::pin(async move { Ok(vec![(category, 0.9)]) })
            });

        let mut period_service = MockPeriodService::default();
        period_service
            .expect_find_similar()
            .returning(|_, _| Box::pin(async move { Ok(vec![]) }));

        let processor =
            ClassifyPipeProcessorImpl::new(Arc::new(adapter), &category_service, &period_service);

        let product = mk_product_with_embedding();
        let product_id = product.product_id;

        let actual = processor.process(vec![product]).await;

        assert!(actual.successes.is_empty());
        assert!(actual.failures.contains(&product_id));
    }

    #[tokio::test]
    async fn should_fail_when_classify_returns_non_candidate_category() {
        let category = mk_category("furniture");
        let period = mk_period("baroque");

        let mut category_service = MockCategoryService::default();
        category_service
            .expect_find_similar()
            .returning(move |_, _| {
                let category = category.clone();
                Box::pin(async move { Ok(vec![(category, 0.9)]) })
            });

        let mut period_service = MockPeriodService::default();
        period_service.expect_find_similar().returning(move |_, _| {
            let period = period.clone();
            Box::pin(async move { Ok(vec![(period, 0.9)]) })
        });

        let mut adapter = MockClassifyAdapter::default();
        adapter.expect_classify().returning(|_| {
            Ok(Batch::try_from(vec![(
                "decorative-objects".to_string(),
                "baroque".to_string(),
            )])
            .unwrap())
        });

        let processor =
            ClassifyPipeProcessorImpl::new(Arc::new(adapter), &category_service, &period_service);

        let product = mk_product_with_embedding();
        let product_id = product.product_id;

        let actual = processor.process(vec![product]).await;

        assert!(actual.successes.is_empty());
        assert!(actual.failures.contains(&product_id));
    }

    #[tokio::test]
    async fn should_fail_when_classify_returns_non_candidate_period() {
        let category = mk_category("furniture");
        let period = mk_period("baroque");

        let mut category_service = MockCategoryService::default();
        category_service
            .expect_find_similar()
            .returning(move |_, _| {
                let category = category.clone();
                Box::pin(async move { Ok(vec![(category, 0.9)]) })
            });

        let mut period_service = MockPeriodService::default();
        period_service.expect_find_similar().returning(move |_, _| {
            let period = period.clone();
            Box::pin(async move { Ok(vec![(period, 0.9)]) })
        });

        let mut adapter = MockClassifyAdapter::default();
        adapter.expect_classify().returning(|_| {
            Ok(Batch::try_from(vec![("furniture".to_string(), "art-deco".to_string())]).unwrap())
        });

        let processor =
            ClassifyPipeProcessorImpl::new(Arc::new(adapter), &category_service, &period_service);

        let product = mk_product_with_embedding();
        let product_id = product.product_id;

        let actual = processor.process(vec![product]).await;

        assert!(actual.successes.is_empty());
        assert!(actual.failures.contains(&product_id));
    }

    #[tokio::test]
    async fn should_fail_batch_when_classify_adapter_errors() {
        let category = mk_category("furniture");
        let period = mk_period("baroque");

        let mut category_service = MockCategoryService::default();
        category_service
            .expect_find_similar()
            .returning(move |_, _| {
                let category = category.clone();
                Box::pin(async move { Ok(vec![(category, 0.9)]) })
            });

        let mut period_service = MockPeriodService::default();
        period_service.expect_find_similar().returning(move |_, _| {
            let period = period.clone();
            Box::pin(async move { Ok(vec![(period, 0.9)]) })
        });

        let mut adapter = MockClassifyAdapter::default();
        adapter
            .expect_classify()
            .returning(|_| Err(PyErr::new::<PyTypeError, _>("Something went wrong")));

        let processor =
            ClassifyPipeProcessorImpl::new(Arc::new(adapter), &category_service, &period_service);

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
        let period = mk_period("baroque");

        let mut category_service = MockCategoryService::default();
        category_service
            .expect_find_similar()
            .times(2)
            .returning(move |_, _| {
                let category = category.clone();
                Box::pin(async move { Ok(vec![(category, 0.9)]) })
            });

        let mut period_service = MockPeriodService::default();
        period_service
            .expect_find_similar()
            .times(2)
            .returning(move |_, _| {
                let period = period.clone();
                Box::pin(async move { Ok(vec![(period, 0.9)]) })
            });

        let adapter = mk_adapter_returning_first_candidates();

        let processor =
            ClassifyPipeProcessorImpl::new(Arc::new(adapter), &category_service, &period_service);

        let mut products = vec![mk_product_with_embedding(), mk_product_with_embedding()];
        let mut missing = mk_product_with_embedding();
        missing.text_embedding = None;
        let missing_id = missing.product_id;
        products.push(missing);

        let actual = processor.process(products).await;

        // 2 products × 2 events each = 4
        assert_eq!(4, actual.successes.len());
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
        let period = mk_period("baroque");

        let mut category_service = MockCategoryService::default();
        category_service
            .expect_find_similar()
            .returning(move |_, _| {
                let category = category.clone();
                Box::pin(async move { Ok(vec![(category, 0.9)]) })
            });

        let mut period_service = MockPeriodService::default();
        period_service.expect_find_similar().returning(move |_, _| {
            let period = period.clone();
            Box::pin(async move { Ok(vec![(period, 0.9)]) })
        });

        let adapter = mk_adapter_returning_first_candidates();

        let processor =
            ClassifyPipeProcessorImpl::new(Arc::new(adapter), &category_service, &period_service);

        let products = fake::vec![Product; count]
            .into_iter()
            .map(|mut product| {
                product.text_embedding = Some(vec![0.1; 4]);
                product
            })
            .collect::<Vec<_>>();

        let actual = processor.process(products).await;

        assert!(actual.failures.is_empty());
        // Each product produces 2 events (category + period)
        assert_eq!(count * 2, actual.successes.len());
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
        let period = mk_period("baroque");

        let mut category_service = MockCategoryService::default();
        category_service
            .expect_find_similar()
            .returning(move |_, _| {
                let category = category.clone();
                Box::pin(async move { Ok(vec![(category, 0.9)]) })
            });

        let mut period_service = MockPeriodService::default();
        period_service.expect_find_similar().returning(move |_, _| {
            let period = period.clone();
            Box::pin(async move { Ok(vec![(period, 0.9)]) })
        });

        let mut adapter = MockClassifyAdapter::default();
        adapter.expect_classify().returning(|batch| {
            if batch.len() == 64 {
                let chosen = batch
                    .iter()
                    .map(|(_, categories, periods)| (categories[0].clone(), periods[0].clone()))
                    .collect::<Vec<_>>();
                Ok(Batch::try_from(chosen).unwrap())
            } else {
                Err(PyErr::new::<PyTypeError, _>("Something went wrong"))
            }
        });

        let processor =
            ClassifyPipeProcessorImpl::new(Arc::new(adapter), &category_service, &period_service);

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
        let period = mk_period("baroque");

        let mut category_service = MockCategoryService::default();
        category_service
            .expect_find_similar()
            .returning(move |_, _| {
                let category = category.clone();
                Box::pin(async move { Ok(vec![(category, 0.9)]) })
            });

        let mut period_service = MockPeriodService::default();
        period_service.expect_find_similar().returning(move |_, _| {
            let period = period.clone();
            Box::pin(async move { Ok(vec![(period, 0.9)]) })
        });

        let mut adapter = MockClassifyAdapter::default();
        adapter
            .expect_classify()
            .returning(|_| Err(PyErr::new::<PyTypeError, _>("Something went wrong")));

        let processor =
            ClassifyPipeProcessorImpl::new(Arc::new(adapter), &category_service, &period_service);

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

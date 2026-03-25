use crate::adapter::ClassifyPeriodAdapter;
use common::{batch::Batch, event_id::EventId};
use product::core::{
    product::Product,
    product_event::{
        ProductEvent, ProductEventPayload,
        enrichment::{
            ClassifiedPeriodProductEnrichmentEventPayload, ProductEnrichmentEventPayload,
        },
    },
};
use product_classification::period::service::PeriodService;
use product_pipeline_common::process::{PipeProcessor, ProcessResult};
use std::{collections::HashSet, sync::Arc};
use time::OffsetDateTime;
use tracing::{error, warn};

pub struct ClassifyPeriodPipeProcesserImpl<'a> {
    classify_period_delegate: Arc<dyn ClassifyPeriodAdapter + Send + Sync>,
    period_service: &'a (dyn PeriodService + Send + Sync),
}

impl<'a> ClassifyPeriodPipeProcesserImpl<'a> {
    pub fn new(
        classify_period_delegate: Arc<dyn ClassifyPeriodAdapter + Send + Sync>,
        period_service: &'a (dyn PeriodService + Send + Sync),
    ) -> Self {
        Self {
            classify_period_delegate,
            period_service,
        }
    }
}

#[async_trait::async_trait]
impl<'a> PipeProcessor for ClassifyPeriodPipeProcesserImpl<'a> {
    async fn process(&self, products: Vec<Product>) -> ProcessResult {
        let mut successes = Vec::with_capacity(products.len());
        let mut failures = HashSet::new();

        // Find candidates
        let mut tie_breaker_inputs = Vec::with_capacity(products.len());
        for product in products {
            if let Some(ref text_embedding) = product.text_embedding {
                let similar_res = self.period_service.find_similar(text_embedding, 5).await;
                match similar_res {
                    Ok(periods) => {
                        if periods.is_empty() {
                            error!(
                                productId = %product.product_id,
                                shopId = %product.shop_id,
                                shopsProductId = %product.shops_product_id,
                                "No similar periods found for product.",
                            );
                            failures.insert(product.product_id);
                        } else {
                            let period_ids: Vec<String> = periods
                                .iter()
                                .map(|(p, _)| p.period_id.to_string())
                                .collect();
                            tie_breaker_inputs.push((product, period_ids));
                        }
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

        // Sort by prompt length so each batch contains items of similar length,
        // reducing padding overhead in transformer models.
        tie_breaker_inputs.sort_by_key(|(product, period_ids)| {
            product.native_title.payload.len() + period_ids.iter().map(|id| id.len()).sum::<usize>()
        });

        // Choose candidate
        for batch in Batch::chunked_from(tie_breaker_inputs.into_iter()) {
            let batch: Batch<_, 64> = batch;
            let in_iter = batch.iter().map(|(product, period_ids)| {
                (product.native_title.payload.to_string(), period_ids.clone())
            });
            let in_batch = Batch::try_from_iter(in_iter)
                .expect("shouldn't fail re-collecting batch of same size");

            let batch_res = self.classify_period_delegate.classify_period(&in_batch);
            match batch_res {
                Ok(batch_periods) => {
                    let local_successes = batch.into_iter().zip(batch_periods).filter_map(
                        |((product, candidates), chosen)| {
                            if candidates.contains(&chosen) {
                                let event = ProductEvent {
                                    aggregate_id: product.product_id,
                                    event_id: EventId::new(),
                                    timestamp: OffsetDateTime::now_utc(),
                                    payload: ProductEventPayload::ProductEnrichmentEvent(
                                        ProductEnrichmentEventPayload::ClassifiedPeriod(
                                            ClassifiedPeriodProductEnrichmentEventPayload {
                                                shop_id: product.shop_id,
                                                shops_product_id: product.shops_product_id,
                                                period_id: chosen.into(),
                                            },
                                        ),
                                    ),
                                };
                                Some(event)
                            } else {
                                warn!(candidates = ?candidates, chosen = chosen, "Tie-Breaker responded with non-candidate period-id.");
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
                        "Failed classifying periods for batch of products.",
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
    use super::ClassifyPeriodPipeProcesserImpl;
    use crate::adapter::MockClassifyPeriodAdapter;
    use common::{
        batch::Batch,
        language::domain::Language,
        period_key::{PeriodId, PeriodKey},
    };
    use fake::{Fake, Faker};
    use product::core::product::Product;
    use product_classification::period::core::Period;
    use product_classification::period::service::MockPeriodService;
    use product_pipeline_common::process::PipeProcessor;
    use pyo3::{PyErr, exceptions::PyTypeError};
    use rstest;
    use serde::de::Error;
    use std::collections::HashMap;
    use std::sync::Arc;
    use time::OffsetDateTime;

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

    #[tokio::test]
    async fn should_classify_period_for_product_with_embedding() {
        let period = mk_period("baroque");

        let mut period_service = MockPeriodService::default();
        period_service.expect_find_similar().returning(move |_, _| {
            let period = period.clone();
            Box::pin(async move { Ok(vec![(period, 0.9)]) })
        });

        let mut adapter = MockClassifyPeriodAdapter::default();
        adapter.expect_classify_period().returning(|batch| {
            let chosen = batch
                .iter()
                .map(|(_, candidates)| candidates[0].clone())
                .collect::<Vec<_>>();
            Ok(Batch::try_from(chosen).unwrap())
        });

        let processor = ClassifyPeriodPipeProcesserImpl::new(Arc::new(adapter), &period_service);

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
            .as_classified_period()
            .unwrap();
        assert_eq!(payload.shop_id, shop_id);
        assert_eq!(payload.shops_product_id, shops_product_id);
        assert_eq!(payload.period_id, PeriodId::from("baroque"));
    }

    #[tokio::test]
    async fn should_fail_when_product_has_no_text_embedding() {
        let mut adapter = MockClassifyPeriodAdapter::default();
        adapter.expect_classify_period().never();

        let mut period_service = MockPeriodService::default();
        period_service.expect_find_similar().never();

        let processor = ClassifyPeriodPipeProcesserImpl::new(Arc::new(adapter), &period_service);

        let mut product: Product = Faker.fake();
        product.text_embedding = None;

        let product_id = product.product_id;
        let actual = processor.process(vec![product]).await;

        assert!(actual.successes.is_empty());
        assert!(actual.failures.contains(&product_id));
    }

    #[tokio::test]
    async fn should_fail_when_find_similar_returns_error() {
        let mut adapter = MockClassifyPeriodAdapter::default();
        adapter.expect_classify_period().never();

        let mut period_service = MockPeriodService::default();
        period_service.expect_find_similar().returning(|_, _| {
            Box::pin(async move {
                Err(opensearch::Error::from(serde_json::Error::custom(
                    "Something went wrong",
                )))
            })
        });

        let processor = ClassifyPeriodPipeProcesserImpl::new(Arc::new(adapter), &period_service);

        let product = mk_product_with_embedding();
        let product_id = product.product_id;

        let actual = processor.process(vec![product]).await;

        assert!(actual.successes.is_empty());
        assert!(actual.failures.contains(&product_id));
    }

    #[tokio::test]
    async fn should_fail_when_find_similar_returns_empty() {
        let mut adapter = MockClassifyPeriodAdapter::default();
        adapter.expect_classify_period().never();

        let mut period_service = MockPeriodService::default();
        period_service
            .expect_find_similar()
            .returning(|_, _| Box::pin(async move { Ok(vec![]) }));

        let processor = ClassifyPeriodPipeProcesserImpl::new(Arc::new(adapter), &period_service);

        let product = mk_product_with_embedding();
        let product_id = product.product_id;

        let actual = processor.process(vec![product]).await;

        assert!(actual.successes.is_empty());
        assert!(actual.failures.contains(&product_id));
    }

    #[tokio::test]
    async fn should_fail_when_classify_period_returns_non_candidate() {
        let period = mk_period("baroque");

        let mut period_service = MockPeriodService::default();
        period_service.expect_find_similar().returning(move |_, _| {
            let period = period.clone();
            Box::pin(async move { Ok(vec![(period, 0.9)]) })
        });

        let mut adapter = MockClassifyPeriodAdapter::default();
        adapter
            .expect_classify_period()
            .returning(|_| Ok(Batch::try_from(vec!["art-deco".to_string()]).unwrap()));

        let processor = ClassifyPeriodPipeProcesserImpl::new(Arc::new(adapter), &period_service);

        let product = mk_product_with_embedding();
        let product_id = product.product_id;

        let actual = processor.process(vec![product]).await;

        assert!(actual.successes.is_empty());
        assert!(actual.failures.contains(&product_id));
    }

    #[tokio::test]
    async fn should_fail_batch_when_classify_period_adapter_errors() {
        let period = mk_period("baroque");

        let mut period_service = MockPeriodService::default();
        period_service.expect_find_similar().returning(move |_, _| {
            let period = period.clone();
            Box::pin(async move { Ok(vec![(period, 0.9)]) })
        });

        let mut adapter = MockClassifyPeriodAdapter::default();
        adapter
            .expect_classify_period()
            .returning(|_| Err(PyErr::new::<PyTypeError, _>("Something went wrong")));

        let processor = ClassifyPeriodPipeProcesserImpl::new(Arc::new(adapter), &period_service);

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
        let period = mk_period("baroque");

        let mut period_service = MockPeriodService::default();
        period_service
            .expect_find_similar()
            .times(2)
            .returning(move |_, _| {
                let period = period.clone();
                Box::pin(async move { Ok(vec![(period, 0.9)]) })
            });

        let mut adapter = MockClassifyPeriodAdapter::default();
        adapter.expect_classify_period().returning(|batch| {
            let chosen = batch
                .iter()
                .map(|(_, candidates)| candidates[0].clone())
                .collect::<Vec<_>>();
            Ok(Batch::try_from(chosen).unwrap())
        });

        let processor = ClassifyPeriodPipeProcesserImpl::new(Arc::new(adapter), &period_service);

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
        let period = mk_period("baroque");

        let mut period_service = MockPeriodService::default();
        period_service.expect_find_similar().returning(move |_, _| {
            let period = period.clone();
            Box::pin(async move { Ok(vec![(period, 0.9)]) })
        });

        let mut adapter = MockClassifyPeriodAdapter::default();
        adapter.expect_classify_period().returning(|batch| {
            let chosen = batch
                .iter()
                .map(|(_, candidates)| candidates[0].clone())
                .collect::<Vec<_>>();
            Ok(Batch::try_from(chosen).unwrap())
        });

        let processor = ClassifyPeriodPipeProcesserImpl::new(Arc::new(adapter), &period_service);

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
        let period = mk_period("baroque");

        let mut period_service = MockPeriodService::default();
        period_service.expect_find_similar().returning(move |_, _| {
            let period = period.clone();
            Box::pin(async move { Ok(vec![(period, 0.9)]) })
        });

        let mut adapter = MockClassifyPeriodAdapter::default();
        adapter.expect_classify_period().returning(|batch| {
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

        let processor = ClassifyPeriodPipeProcesserImpl::new(Arc::new(adapter), &period_service);

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
        let period = mk_period("baroque");

        let mut period_service = MockPeriodService::default();
        period_service.expect_find_similar().returning(move |_, _| {
            let period = period.clone();
            Box::pin(async move { Ok(vec![(period, 0.9)]) })
        });

        let mut adapter = MockClassifyPeriodAdapter::default();
        adapter
            .expect_classify_period()
            .returning(|_| Err(PyErr::new::<PyTypeError, _>("Something went wrong")));

        let processor = ClassifyPeriodPipeProcesserImpl::new(Arc::new(adapter), &period_service);

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

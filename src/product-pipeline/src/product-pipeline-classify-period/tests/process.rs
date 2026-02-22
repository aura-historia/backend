use common::{
    language::domain::Language,
    period_key::{PeriodId, PeriodKey},
};
use fake::{Fake, Faker};
use product::core::product::Product;
use product_classification::period::core::Period;
use product_classification::period::service::MockPeriodService;
use product_pipeline_classify_period::{
    adapter::ClassifyPeriodAdapterImpl, process::ClassifyPeriodPipeProcesserImpl,
};
use product_pipeline_common::process::PipeProcessor;
use std::collections::HashMap;
use std::sync::Arc;
use time::OffsetDateTime;

fn mk_period(period_id: &str) -> Period {
    let mut display_name = HashMap::new();
    display_name.insert(Language::En, period_id.into());
    Period {
        period_id: PeriodId::raw(period_id),
        period_key: PeriodKey::from(format!("{period_id}-key")),
        meta_name: "meta-name".into(),
        meta_description: "meta-description".into(),
        meta_keywords: Default::default(),
        embedding: vec![0.1; 1024],
        display_name,
        display_description: Default::default(),
        created: OffsetDateTime::now_utc(),
        updated: OffsetDateTime::now_utc(),
    }
}

#[tokio::test]
#[serial_test::serial]
async fn should_process_classification() {
    let adapter = ClassifyPeriodAdapterImpl::new().unwrap();

    let baroque_period = mk_period("baroque");
    let art_nouveau_period = mk_period("art-nouveau");
    let renaissance_period = mk_period("renaissance");
    let mut period_service = MockPeriodService::default();
    period_service
        .expect_find_similar()
        .return_once(move |_, _| {
            let baroque_period = baroque_period.clone();
            let art_nouveau_period = art_nouveau_period.clone();
            let renaissance_period = renaissance_period.clone();
            Box::pin(async move {
                Ok(vec![
                    (art_nouveau_period, 0.8),
                    (baroque_period, 0.9),
                    (renaissance_period, 0.88),
                ])
            })
        });

    let processor = ClassifyPeriodPipeProcesserImpl::new(Arc::new(adapter), &period_service);

    let mut product = Faker.fake::<Product>();
    product.native_title.payload = "Antique Chair".into();
    product.text_embedding = Some(vec![0.1; 1024]);

    let actual = processor.process(vec![product]).await;

    assert!(actual.failures.is_empty());
    assert_eq!(1, actual.successes.len());

    let payload = actual.successes[0]
        .payload
        .as_enrichment_event()
        .unwrap()
        .as_classified_period()
        .unwrap();
    assert_eq!(payload.period_id, PeriodId::from("baroque"));
}

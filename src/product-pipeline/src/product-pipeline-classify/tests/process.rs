use common::{
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
use product_pipeline_classify::{
    adapter::ClassifyAdapterImpl, process::ClassifyPipeProcessorImpl,
};
use product_pipeline_common::process::PipeProcessor;
use std::collections::HashMap;
use std::sync::Arc;
use time::OffsetDateTime;

fn mk_category(category_id: &str) -> Category {
    let mut display_name = HashMap::new();
    display_name.insert(Language::En, category_id.into());
    Category {
        category_id: CategoryId::raw(category_id),
        category_key: CategoryKey::from(format!("{category_id}-key")),
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
    let adapter = ClassifyAdapterImpl::new().unwrap();

    let furniture_category = mk_category("furniture");
    let musical_instruments_category = mk_category("musical-instruments");
    let militaria_category = mk_category("militaria");
    let mut category_service = MockCategoryService::default();
    category_service
        .expect_find_similar()
        .return_once(move |_, _| {
            let furniture_category = furniture_category.clone();
            let musical_instruments_category = musical_instruments_category.clone();
            let militaria_category = militaria_category.clone();
            Box::pin(async move {
                Ok(vec![
                    (musical_instruments_category, 0.8),
                    (furniture_category, 0.9),
                    (militaria_category, 0.88),
                ])
            })
        });

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

    let processor = ClassifyPipeProcessorImpl::new(
        Arc::new(adapter),
        &category_service,
        &period_service,
    );

    let mut product = Faker.fake::<Product>();
    product.native_title.payload = "Antique Baroque Chair".into();
    product.text_embedding = Some(vec![0.1; 1024]);

    let actual = processor.process(vec![product]).await;

    assert!(actual.failures.is_empty());
    assert_eq!(2, actual.successes.len());

    let category_payload = actual.successes[0]
        .payload
        .as_enrichment_event()
        .unwrap()
        .as_classified_category()
        .unwrap();
    assert_eq!(category_payload.category_id, CategoryId::from("furniture"));

    let period_payload = actual.successes[1]
        .payload
        .as_enrichment_event()
        .unwrap()
        .as_classified_period()
        .unwrap();
    assert_eq!(period_payload.period_id, PeriodId::from("baroque"));
}

use common::product_id::ProductId;
use common::{language::domain::Language, localized::Localized};
use fake::rand::seq::SliceRandom;
use fake::{Fake, Faker};
use product::core::product::Product;
use product_pipeline_common::process::PipeProcessor;
use product_pipeline_extract_attribute::{
    adapter::ExtractionAdapterImpl, process::AttributeExtractionPipeProcesserImpl,
};
use std::sync::Arc;

#[rstest::rstest]
#[trace]
#[case(0)]
#[serial_test::serial]
#[tokio::test]
async fn should_process_extraction(#[case] count: usize) {
    let adapter = ExtractionAdapterImpl::new().unwrap();
    let extraction_pipe_processor = AttributeExtractionPipeProcesserImpl::new(Arc::new(adapter));

    let product_id = ProductId::new();
    let mut products = fake::vec![Product; count];
    let mut product = Faker.fake::<Product>();
    product.product_id = product_id;
    product.native_title.payload = "Antique Chair".into();
    product.native_title.localization = Language::En;
    product.native_description = Some(Localized {
        localization: Language::En,
        payload: "oak 1845. no nazi".into(),
    });

    products.push(product);
    products.shuffle(&mut fake::rand::rng());

    let actual = extraction_pipe_processor.process(products).await;
    assert!(actual.failures.is_empty());
    // 1 enrichment + 1 policy (no nazi)
    assert_eq!(count + 2, actual.successes.len());

    let event = actual
        .successes
        .into_iter()
        .find(|event| {
            event.aggregate_id == product_id && event.payload.as_enrichment_event().is_some()
        })
        .unwrap();

    let extracted = event
        .payload
        .as_enrichment_event()
        .unwrap()
        .as_extracted_attributes()
        .unwrap();
    let has_extracted_year = extracted.origin_year.is_some_and(|y| y == 1845.into())
        || extracted.origin_year_min.is_some_and(|y| y == 1845.into())
        || extracted.origin_year_max.is_some_and(|y| y == 1845.into());
    assert!(has_extracted_year);
}

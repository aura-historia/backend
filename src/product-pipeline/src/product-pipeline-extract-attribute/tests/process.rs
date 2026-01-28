use common::language::record::LanguageRecord;
use common::{language::record::TextRecord, product_id::ProductId};
use fake::rand::seq::SliceRandom;
use fake::{Fake, Faker};
use product_pipeline_common::{process::PipeProcessor, types::TextEmbeddedPipeProduct};
use product_pipeline_extract_attribute::{
    adapter::ExtractionAdapterImpl, process::AttributeExtractionPipeProcesserImpl,
};
use std::sync::Arc;

#[rstest::rstest]
#[trace]
#[case(0)]
#[serial_test::serial]
fn should_process_extraction(#[case] count: usize) {
    // Set environment variable to use CPU for testing
    unsafe {
        std::env::set_var("AURA_DEVICE", "cpu");
    }

    let adapter = ExtractionAdapterImpl::new().unwrap();
    let extraction_pipe_processor = AttributeExtractionPipeProcesserImpl::new(Arc::new(adapter));

    let product_id = ProductId::new();
    let mut products = fake::vec![TextEmbeddedPipeProduct; count];
    let mut product = Faker.fake::<TextEmbeddedPipeProduct>();
    product.product_id = product_id;
    // Use very short text to minimize test duration
    product.native_title.text = "Antique Chair".to_owned();
    product.native_title.language = LanguageRecord::En;
    product.native_description = Some(TextRecord {
        language: LanguageRecord::En,
        text: "1800s oak".to_owned(),
    });
    // Generate a small fake embedding
    product.text_embedding = vec![0.1; 1024];
    products.push(product);
    products.shuffle(&mut fake::rand::rng());

    let actual = extraction_pipe_processor.process(products);
    assert!(actual.failures.is_empty());
    assert_eq!(count + 1, actual.successes.len());

    let extracted = actual
        .successes
        .into_iter()
        .find(|out_product| out_product.product_id == product_id)
        .unwrap();

    // Verify that extraction was performed - check if any attribute has a non-default value
    // These fields use enum types with Unknown as the default, or Option types for years
    let has_extracted_data = extracted.origin_year.is_some()
        || extracted.origin_year_min.is_some()
        || extracted.origin_year_max.is_some()
        || !matches!(
            extracted.authenticity,
            product::dynamodb::authenticity_record::AuthenticityRecord::Unknown
        )
        || !matches!(
            extracted.condition,
            product::dynamodb::condition_record::ConditionRecord::Unknown
        )
        || !matches!(
            extracted.provenance,
            product::dynamodb::provenance_record::ProvenanceRecord::Unknown
        )
        || !matches!(
            extracted.restoration,
            product::dynamodb::restoration_record::RestorationRecord::Unknown
        );

    assert!(
        has_extracted_data,
        "Expected at least one attribute to be extracted"
    );

    // Verify the native title is preserved
    assert_eq!("Antique Chair", extracted.native_title.text);
    assert_eq!(LanguageRecord::En, extracted.native_title.language);
}

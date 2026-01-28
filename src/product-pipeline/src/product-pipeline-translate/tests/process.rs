use common::language::record::LanguageRecord;
use common::{language::record::TextRecord, product_id::ProductId};
use fake::rand::seq::SliceRandom;
use fake::{Fake, Faker};
use product_pipeline_common::{process::PipeProcessor, types::CleansedPipeProduct};
use product_pipeline_translate::{
    adapter::TranslationAdapterImpl, process::TranslationPipeProcesserImpl,
};
use std::sync::Arc;

#[rstest::rstest]
#[trace]
#[case(0)]
#[serial_test::serial]
fn should_process_translation(#[case] count: usize) {
    // Set environment variable to use CPU for testing
    std::env::set_var("AURA_DEVICE", "cpu");

    let adapter = TranslationAdapterImpl::new().unwrap();
    let translation_pipe_processor = TranslationPipeProcesserImpl::new(Arc::new(adapter));

    let product_id = ProductId::new();
    let mut products = fake::vec![CleansedPipeProduct; count];
    let mut product = Faker.fake::<CleansedPipeProduct>();
    product.product_id = product_id;
    // Use very short text to minimize test duration
    product.native_title.text = "Test".to_owned();
    product.native_title.language = LanguageRecord::De;
    product.native_description = Some(TextRecord {
        language: LanguageRecord::De,
        text: "Beispiel".to_owned(),
    });
    products.push(product);
    products.shuffle(&mut fake::rand::rng());

    let actual = translation_pipe_processor.process(products);
    assert!(actual.failures.is_empty());
    assert_eq!(count + 1, actual.successes.len());

    let translated = actual
        .successes
        .into_iter()
        .find(|out_product| out_product.product_id == product_id)
        .unwrap();

    // Verify that translations were produced (at least one language)
    assert!(
        !translated.other_title.is_empty(),
        "Expected translations to be generated"
    );

    // Verify the native title is preserved
    assert_eq!("Test", translated.native_title.text);
    assert_eq!(LanguageRecord::De, translated.native_title.language);
}

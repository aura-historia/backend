use common::language::domain::Language;
use common::language::record::LanguageRecord;
use common::{language::record::TextRecord, product_id::ProductId};
use fake::rand::seq::SliceRandom;
use fake::{Fake, Faker};
use product_pipeline_common::{process::PipeProcessor, types::CleansedPipeProduct};
use product_pipeline_translate::{
    adapter::TranslationAdapterImpl, process::TranslationPipeProcesserImpl,
};
use std::sync::Arc;
use strum::EnumCount;

#[rstest::rstest]
#[trace]
#[case(0)]
#[serial_test::serial]
fn should_process_translation(#[case] count: usize) {
    let adapter = TranslationAdapterImpl::new().unwrap();
    let translation_pipe_processor = TranslationPipeProcesserImpl::new(Arc::new(adapter));

    let product_id = ProductId::new();
    let mut products = fake::vec![CleansedPipeProduct; count];
    let mut product = Faker.fake::<CleansedPipeProduct>();
    product.product_id = product_id;
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

    assert_eq!(Language::COUNT - 1, translated.other_title.len(),);
    assert_eq!("Test", translated.native_title.text);
    assert_eq!(LanguageRecord::De, translated.native_title.language);
}

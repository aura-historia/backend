use common::language::domain::Language;
use common::localized::Localized;
use common::product_id::ProductId;
use fake::{Fake, Faker};
use product::core::product::Product;
use product_pipeline_common::process::PipeProcessor;
use product_pipeline_translate::{
    adapter::TranslationAdapterImpl, process::TranslationPipeProcesserImpl,
};
use std::sync::Arc;

#[test]
#[serial_test::serial]
fn should_process_translation() {
    let adapter = TranslationAdapterImpl::new().unwrap();
    let translation_pipe_processor = TranslationPipeProcesserImpl::new(Arc::new(adapter));

    let product_id = ProductId::new();
    let mut product = Faker.fake::<Product>();
    product.product_id = product_id;
    product.native_title.payload = "Test".into();
    product.native_title.localization = Language::De;
    product.native_description = Some(Localized {
        localization: Language::De,
        payload: "Beispiel".into(),
    });

    let actual = translation_pipe_processor.process(vec![product]);
    assert!(actual.failures.is_empty());
    assert_eq!(6, actual.successes.len());
}

use common::{
    language::record::{LanguageRecord, TextRecord},
    product_id::ProductId,
};
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
#[case(1)]
#[case(7)]
#[case(15)]
#[case(16)]
#[case(17)]
#[case(21)]
#[case(31)]
#[case(42)]
#[case(63)]
#[case(64)]
#[case(65)]
#[case(69)]
#[case(144)]
fn should_process_text_embedding(#[case] count: usize) {
    let adapter = TranslationAdapterImpl::new().unwrap();
    let translation_pipe_processor = TranslationPipeProcesserImpl::new(Arc::new(adapter));

    let product_id = ProductId::new();
    let mut products = fake::vec![CleansedPipeProduct; count];
    let product = CleansedPipeProduct {
        product_id,
        shop_id: Faker.fake(),
        shops_product_id: Faker.fake(),
        shop_name: Faker.fake(),
        native_title: TextRecord {
            text: "blue".to_owned(),
            language: LanguageRecord::En,
        },
        native_description: Some(TextRecord {
            text: "Hallo Welt!".to_owned(),
            language: LanguageRecord::De,
        }),
    };
    products.push(product);
    products.shuffle(&mut fake::rand::rng());

    let actual = translation_pipe_processor.process(products);
    assert!(actual.failures.is_empty());
    assert_eq!(count + 1, actual.successes.len());

    let expected = actual
        .successes
        .into_iter()
        .find(|out_product| out_product.product_id == product_id)
        .unwrap();

    // Title
    assert_eq!(LanguageRecord::En, expected.native_title.language);
    assert_eq!("blue", expected.native_title.text.to_lowercase());
    assert_eq!(
        "blau",
        expected
            .other_title
            .get(&LanguageRecord::De)
            .unwrap()
            .to_lowercase()
    );
    assert_eq!(
        "bleu",
        expected
            .other_title
            .get(&LanguageRecord::Fr)
            .unwrap()
            .to_lowercase()
    );
    assert_eq!(
        "azul",
        &expected
            .other_title
            .get(&LanguageRecord::Es)
            .unwrap()
            .to_lowercase()
    );

    // Description
    assert_eq!(
        LanguageRecord::De,
        expected.native_description.as_ref().unwrap().language
    );
    assert_eq!(
        "Hallo Welt!",
        expected.native_description.as_ref().unwrap().text
    );
    assert_eq!(
        &"Hello world!",
        expected
            .other_description
            .get(&LanguageRecord::En)
            .as_ref()
            .unwrap()
    );
    let description_fr = expected
        .other_description
        .get(&LanguageRecord::Fr)
        .as_ref()
        .unwrap()
        .to_lowercase();
    assert!(description_fr.contains("bonjour"));
    assert!(description_fr.contains("monde"));

    let description_es = expected
        .other_description
        .get(&LanguageRecord::Es)
        .as_ref()
        .unwrap()
        .to_lowercase();
    assert!(description_es.contains("hola"));
    assert!(description_es.contains("mundo"));
}

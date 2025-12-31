use common::{
    language::record::{LanguageRecord, TextRecord},
    product_id::ProductId,
    year::Year,
};
use fake::{Fake, Faker};
use product::dynamodb::{
    authenticity_record::AuthenticityRecord, condition_record::ConditionRecord,
    provenance_record::ProvenanceRecord, restoration_record::RestorationRecord,
};
use product_pipeline_common::process::PipeProcessor;
use product_pipeline_common::types::TextEmbeddedPipeProduct;
use product_pipeline_extract_attribute::{
    adapter::ExtractionAdapterImpl, process::AttributeExtractionPipeProcesserImpl,
};
use std::sync::Arc;

#[test]
fn should_process_attribute_extraction() {
    let adapter = ExtractionAdapterImpl::new().unwrap();
    let processor = AttributeExtractionPipeProcesserImpl::new(Arc::new(adapter));

    let product_id = ProductId::new();
    let product = TextEmbeddedPipeProduct {
        product_id,
        shop_id: Faker.fake(),
        shops_product_id: Faker.fake(),
        native_title: TextRecord {
            text: "Bauernschrank in originalem und exzellentem Zustand ohne jegliche Gebrauchsspuren, nicht restauriert, Odenwald, 1842".to_owned(),
            language: LanguageRecord::De,
        },
        other_title: Default::default(),
        native_description: None,
        other_description: Default::default(),
        text_embedding: vec![],
    };

    let actual = processor.process(vec![product]);
    assert!(actual.failures.is_empty());
    assert_eq!(1, actual.successes.len());

    let expected = actual
        .successes
        .into_iter()
        .find(|out_product| out_product.product_id == product_id)
        .unwrap();

    assert_eq!(Year::from(1842), expected.origin_year.unwrap());
    assert_eq!(AuthenticityRecord::Original, expected.authenticity);
    assert_eq!(ConditionRecord::Excellent, expected.condition);
    assert_eq!(ProvenanceRecord::Unknown, expected.provenance);
    assert_eq!(RestorationRecord::None, expected.restoration);
}

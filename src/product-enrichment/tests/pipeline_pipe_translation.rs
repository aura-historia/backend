use common::{language::domain::Language, localized::Localized};
use fake::{Fake, Faker, rand::seq::SliceRandom};
use product::core::product_event::ProductCreatedEventPayload;
use product_enrichment::{
    pipeline::{
        pipe::{EnrichmentPipe, PipeProduct, PipeProductSource},
        translate::TranslationEnrichmentPipeImpl,
    },
    translate::TranslationDelegateImpl,
};
use std::sync::Arc;
use test_api::*;

#[tokio::test]
async fn should_translate_title_and_description() {
    let mut products = fake::vec![ProductCreatedEventPayload; 69]
        .into_iter()
        .map(|payload| PipeProduct {
            source: PipeProductSource {
                product_id: Faker.fake(),
                payload,
            },
            update: Default::default(),
        })
        .collect::<Vec<_>>();
    for product in &mut products {
        product.source.payload.native_title = Localized {
            localization: Language::De,
            payload: Faker.fake(),
        };
        product.source.payload.native_description = None;
    }

    let expected_pipe_product_1 = PipeProduct {
        source: PipeProductSource {
            product_id: Faker.fake(),
            payload: ProductCreatedEventPayload {
                shop_id: Faker.fake(),
                shops_product_id: Faker.fake(),
                shop_name: Faker.fake(),
                native_title: Localized {
                    localization: Language::De,
                    payload: "Hallo Welt".into(),
                },
                native_description: Some(Localized {
                    localization: Language::En,
                    payload: "The earth is a planet!".into(),
                }),
                native_price: Faker.fake(),
                other_price: Faker.fake(),
                state: Faker.fake(),
                url: Faker.fake(),
                images: Faker.fake(),
            },
        },
        update: Default::default(),
    };
    let expected_pipe_product_2 = PipeProduct {
        source: PipeProductSource {
            product_id: Faker.fake(),
            payload: ProductCreatedEventPayload {
                shop_id: Faker.fake(),
                shops_product_id: Faker.fake(),
                shop_name: Faker.fake(),
                native_title: Localized {
                    localization: Language::En,
                    payload: "Today is friday.".into(),
                },
                native_description: Some(Localized {
                    localization: Language::De,
                    payload: "Ihr Name ist Martha".into(),
                }),
                native_price: Faker.fake(),
                other_price: Faker.fake(),
                state: Faker.fake(),
                url: Faker.fake(),
                images: Faker.fake(),
            },
        },
        update: Default::default(),
    };
    products.push(expected_pipe_product_1.clone());
    products.push(expected_pipe_product_2.clone());
    products.shuffle(&mut fake::rand::rng());

    let translation_delegate = TranslationDelegateImpl::new().unwrap();
    let translation_pipe = TranslationEnrichmentPipeImpl::new(Arc::new(translation_delegate));
    let actual = translation_pipe.enrich(products);

    assert!(actual.failures.is_empty());
    assert_eq!(71, actual.successes.len());

    let actual_1 = actual
        .successes
        .iter()
        .find(|success| success.source.product_id == expected_pipe_product_1.source.product_id)
        .unwrap();
    assert_eq!(
        "Hello world",
        actual_1.update.document.clone().unwrap().title_en.unwrap()
    );
    assert_eq!(
        "Die Erde ist ein Planet!",
        actual_1
            .update
            .document
            .clone()
            .unwrap()
            .description_de
            .unwrap()
    );

    let actual_2 = actual
        .successes
        .iter()
        .find(|success| success.source.product_id == expected_pipe_product_2.source.product_id)
        .unwrap();
    assert_eq!(
        "Heute ist Freitag.",
        actual_2.update.document.clone().unwrap().title_de.unwrap()
    );
    assert_eq!(
        "Her name is Martha",
        actual_2
            .update
            .document
            .clone()
            .unwrap()
            .description_en
            .unwrap()
    );
}

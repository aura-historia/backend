use common::currency::domain::Currency;
use fxrate::dynamodb::{
    record::{FxRatesRecord, mk_pk, mk_sk, rate_key},
    repository::{FxRateDynamoDbRepository, FxRateDynamoDbRepositoryImpl},
};
use std::collections::HashMap;
use strum::IntoEnumIterator;
use test_api::*;
use time::OffsetDateTime;

#[aura_integration_test(services = [DynamoDB()])]
async fn should_get_none_when_not_exists() {
    let repository = FxRateDynamoDbRepositoryImpl::new(get_dynamodb_client().await, "table_1");

    let actual = repository.get_fx_rates_record().await.unwrap();

    assert!(actual.is_none());
}

#[aura_integration_test(services = [DynamoDB()])]
async fn should_put_then_get_some_when_exists() {
    let mut rates = HashMap::new();
    for src in Currency::iter() {
        for tgt in Currency::iter() {
            if src != tgt {
                rates.insert(rate_key(&src, &tgt), 1_234_567u64);
            }
        }
    }
    let fx_rates_record = FxRatesRecord {
        pk: mk_pk().to_owned(),
        sk: mk_sk().to_owned(),
        rates,
        timestamp: OffsetDateTime::now_utc(),
    };
    let repository = FxRateDynamoDbRepositoryImpl::new(get_dynamodb_client().await, "table_1");
    let _ = repository
        .put_fx_rates_record(fx_rates_record.clone())
        .await
        .unwrap();

    let actual = repository.get_fx_rates_record().await.unwrap().unwrap();

    assert_eq!(fx_rates_record, actual);
}

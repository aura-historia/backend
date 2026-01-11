use fxrate::dynamodb::{
    record::{FxRatesRecord, mk_pk, mk_sk},
    repository::{FxRateDynamoDbRepository, FxRateDynamoDbRepositoryImpl},
};
use test_api::*;
use time::OffsetDateTime;

#[localstack_test(services = [DynamoDB()])]
async fn should_get_none_when_not_exists() {
    let repository = FxRateDynamoDbRepositoryImpl::new(get_dynamodb_client().await, "table_1");

    let actual = repository.get_fx_rates_record().await.unwrap();

    assert!(actual.is_none());
}

#[localstack_test(services = [DynamoDB()])]
async fn should_put_then_get_some_when_exists() {
    let fx_rates_record = FxRatesRecord {
        pk: mk_pk().to_owned(),
        sk: mk_sk().to_owned(),
        eur_gbp: 1_234_567,
        eur_usd: 1_234_567,
        eur_aud: 1_234_567,
        eur_cad: 1_234_567,
        eur_nzd: 1_234_567,
        gbp_eur: 1_234_567,
        gbp_usd: 1_234_567,
        gbp_aud: 1_234_567,
        gbp_cad: 1_234_567,
        gbp_nzd: 1_234_567,
        usd_eur: 1_234_567,
        usd_gbp: 1_234_567,
        usd_aud: 1_234_567,
        usd_cad: 1_234_567,
        usd_nzd: 1_234_567,
        aud_eur: 1_234_567,
        aud_gbp: 1_234_567,
        aud_usd: 1_234_567,
        aud_cad: 1_234_567,
        aud_nzd: 1_234_567,
        cad_eur: 1_234_567,
        cad_gbp: 1_234_567,
        cad_usd: 1_234_567,
        cad_aud: 1_234_567,
        cad_nzd: 1_234_567,
        nzd_eur: 1_234_567,
        nzd_gbp: 1_234_669,
        nzd_usd: 1_234_567,
        nzd_aud: 10_234_567,
        nzd_cad: 85_234_678,
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

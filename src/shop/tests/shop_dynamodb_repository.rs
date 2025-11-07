use aws_sdk_dynamodb::error::SdkError;
use common::shop_id::{ShopId, ShopIdentifier};
use fake::{Fake, Faker};
use shop::core::shop::Shop;
use shop::dynamodb::{
    repository::{ShopDynamoDbRepository, ShopDynamoDbRepositoryImpl},
    shop_record::ShopRecord,
};
use test_api::*;
use url::Url;

async fn get_repository() -> ShopDynamoDbRepositoryImpl<'static> {
    ShopDynamoDbRepositoryImpl::new(get_dynamodb_client().await, "table_1")
}

#[localstack_test(services = [DynamoDB()])]
async fn should_return_none_when_shop_record_not_exists_for_get_by_id() {
    let repository = get_repository().await;

    let actual = repository
        .get_shop_record_by_id(&Faker.fake())
        .await
        .unwrap();

    assert!(actual.is_none());
}

#[localstack_test(services = [DynamoDB()])]
async fn should_return_none_when_shop_record_not_exists_for_get_by_url() {
    let repository = get_repository().await;

    let actual = repository
        .get_shop_record_by_url(&Url::parse("https://google.com").unwrap())
        .await
        .unwrap();

    assert!(actual.is_none());
}

#[localstack_test(services = [DynamoDB()])]
async fn should_return_some_when_shop_record_exists_for_get_by_id() {
    let repository = get_repository().await;

    let expected = ShopRecord::from_shop_as_shop_id_record(Faker.fake::<Shop>());
    let _ = repository.put_shop_record(expected.clone()).await.unwrap();
    let actual = repository
        .get_shop_record_by_id(&expected.shop_id)
        .await
        .unwrap()
        .unwrap();

    assert_eq!(expected, actual);
}

#[localstack_test(services = [DynamoDB()])]
async fn should_return_some_when_shop_record_exists_for_get_by_url() {
    let repository = get_repository().await;

    let records =
        ShopRecord::try_clone_from_shop_as_shop_url_records(&Faker.fake::<Shop>()).unwrap();
    let _ = repository
        .put_shop_records_transact(records.clone())
        .await
        .unwrap();
    let actual = repository
        .get_shop_record_by_url(&records[0].urls[0])
        .await
        .unwrap()
        .unwrap();

    assert_eq!(records[0], actual);
}

#[localstack_test(services = [DynamoDB()])]
async fn should_succeed_transact_write_shop_records_when_none_exist() {
    let repository = get_repository().await;

    let records =
        ShopRecord::try_clone_from_shop_as_shop_url_records(&Faker.fake::<Shop>()).unwrap();
    let _ = repository
        .put_shop_records_transact(records.clone())
        .await
        .unwrap();

    for record in records {
        let actual = repository
            .get_shop_record_by_url(&record.urls[0])
            .await
            .unwrap()
            .unwrap();
        assert_eq!(record, actual);
    }
}

#[localstack_test(services = [DynamoDB()])]
async fn should_succeed_transact_write_shop_records_when_none_with_differing_shop_id_exist() {
    let repository = get_repository().await;

    let records =
        ShopRecord::try_clone_from_shop_as_shop_url_records(&Faker.fake::<Shop>()).unwrap();

    // first write
    let _ = repository
        .put_shop_records_transact(records.clone())
        .await
        .unwrap();
    for record in records.clone() {
        let actual = repository
            .get_shop_record_by_url(&record.urls[0])
            .await
            .unwrap()
            .unwrap();
        assert_eq!(record, actual);
    }

    // now they exist - same shop_id - so overwrite allowed
    let _ = repository
        .put_shop_records_transact(records.clone())
        .await
        .unwrap();

    for record in records {
        let actual = repository
            .get_shop_record_by_url(&record.urls[0])
            .await
            .unwrap()
            .unwrap();
        assert_eq!(record, actual);
    }
}

#[localstack_test(services = [DynamoDB()])]
async fn should_succeed_transact_write_shop_records_when_some_with_differing_shop_id_exist() {
    let repository = get_repository().await;

    let records =
        ShopRecord::try_clone_from_shop_as_shop_url_records(&Faker.fake::<Shop>()).unwrap();

    // first write
    let _ = repository
        .put_shop_records_transact(records.clone())
        .await
        .unwrap();
    for record in records.clone() {
        let actual = repository
            .get_shop_record_by_url(&record.urls[0])
            .await
            .unwrap()
            .unwrap();
        assert_eq!(record, actual);
    }

    let mut records = records.clone();
    for record in &mut records {
        record.shop_id = ShopId::new();
    }

    // now they exist - different shop_id - so overwrite forbidden
    let write_res = repository
        .put_shop_records_transact(records.clone())
        .await
        .unwrap_err();
    match write_res {
        SdkError::ServiceError(service_error) => {
            assert!(service_error.into_err().is_transaction_canceled_exception())
        }
        other => panic!(
            "Expected 'SdkError::ServiceError(TransactWriteItemsError::TransactionCanceledException(_))' but got '{other:?}'"
        ),
    }
}

#[localstack_test(services = [DynamoDB()])]
async fn should_get_shop_records() {
    let repository = get_repository().await;

    let shop = Faker.fake::<Shop>();
    let mut expected = ShopRecord::try_clone_from_shop_as_shop_url_records(&shop).unwrap();
    let record_with_shop_id_pk = ShopRecord::from_shop_as_shop_id_record(shop);
    expected.push(record_with_shop_id_pk.clone());

    let mut shop_identifiers = record_with_shop_id_pk
        .urls
        .iter()
        .map(|url| ShopIdentifier::from(url.clone()))
        .collect::<Vec<_>>();
    shop_identifiers.push(record_with_shop_id_pk.shop_id.into());

    let _ = repository
        .put_shop_records_transact(expected.clone())
        .await
        .unwrap();

    let actual = repository
        .get_shop_records(&shop_identifiers.try_into().unwrap())
        .await
        .unwrap();
    assert!(actual.unprocessed.is_none());
    assert_eq!(expected.len(), actual.items.len());
    assert!(actual.items.iter().all(|actual| expected.contains(actual)));
}

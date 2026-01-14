use std::collections::HashMap;

use aws_sdk_dynamodb::error::SdkError;
use common::domain::Domain;
use common::shop_id::{ShopId, ShopIdentifier};
use common::shop_name::ShopName;
use fake::{Fake, Faker};
use shop::core::shop::Shop;
use shop::dynamodb::shop_record_update::ShopRecordUpdate;
use shop::dynamodb::{
    repository::{ShopDynamoDbRepository, ShopDynamoDbRepositoryImpl},
    shop_record::ShopRecord,
};
use test_api::*;
use time::OffsetDateTime;
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
        .get_shop_record_by_domain(&Domain::try_from("https://google.com").unwrap())
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

    let records = ShopRecord::clone_from_shop_as_shop_domain_records(&Faker.fake::<Shop>());
    let _ = repository
        .put_shop_records_transact(records.clone())
        .await
        .unwrap();
    let actual = repository
        .get_shop_record_by_domain(records[0].domains.iter().next().unwrap())
        .await
        .unwrap()
        .unwrap();

    assert_eq!(records[0], actual);
}

#[localstack_test(services = [DynamoDB()])]
async fn should_succeed_transact_write_shop_records_when_none_exist() {
    let repository = get_repository().await;

    let records = ShopRecord::clone_from_shop_as_shop_domain_records(&Faker.fake::<Shop>());
    let _ = repository
        .put_shop_records_transact(records.clone())
        .await
        .unwrap();

    for record in records {
        let actual = repository
            .get_shop_record_by_domain(record.domains.iter().next().unwrap())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(record, actual);
    }
}

#[localstack_test(services = [DynamoDB()])]
async fn should_succeed_transact_write_shop_records_when_none_with_differing_shop_id_exist() {
    let repository = get_repository().await;

    let records = ShopRecord::clone_from_shop_as_shop_domain_records(&Faker.fake::<Shop>());

    // first write
    let _ = repository
        .put_shop_records_transact(records.clone())
        .await
        .unwrap();
    for record in records.clone() {
        let actual = repository
            .get_shop_record_by_domain(record.domains.iter().next().unwrap())
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
            .get_shop_record_by_domain(record.domains.iter().next().unwrap())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(record, actual);
    }
}

#[localstack_test(services = [DynamoDB()])]
async fn should_fail_transact_write_shop_records_when_some_with_differing_shop_id_exist() {
    let repository = get_repository().await;

    let records = ShopRecord::clone_from_shop_as_shop_domain_records(&Faker.fake::<Shop>());

    // first write
    let _ = repository
        .put_shop_records_transact(records.clone())
        .await
        .unwrap();
    for record in records.clone() {
        let actual = repository
            .get_shop_record_by_domain(record.domains.iter().next().unwrap())
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
    let mut expected = ShopRecord::clone_from_shop_as_shop_domain_records(&shop);
    let record_with_shop_id_pk = ShopRecord::from_shop_as_shop_id_record(shop);
    expected.push(record_with_shop_id_pk.clone());

    let mut shop_identifiers = record_with_shop_id_pk
        .domains
        .iter()
        .map(|domain| ShopIdentifier::from(domain.clone()))
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

#[localstack_test(services = [DynamoDB()])]
async fn should_transact_write() {
    let repository = get_repository().await;

    let shop = Shop {
        shop_id: Faker.fake(),
        name: Faker.fake(),
        shop_type: Faker.fake(),
        domains: [
            Domain::try_from("https://foo.de").unwrap(),
            Domain::try_from("https://foo.com").unwrap(),
            Domain::try_from("https://foo.en").unwrap(),
        ]
        .into(),
        image: Faker.fake(),
        created: OffsetDateTime::now_utc(),
        updated: OffsetDateTime::now_utc(),
    };
    let mut existing_shop_records = ShopRecord::clone_from_shop_as_shop_domain_records(&shop);
    let record_with_shop_id_pk = ShopRecord::from_shop_as_shop_id_record(shop.clone());
    existing_shop_records.push(record_with_shop_id_pk.clone());

    let mut shop_identifiers = record_with_shop_id_pk
        .domains
        .iter()
        .map(|domain| ShopIdentifier::from(domain.clone()))
        .collect::<Vec<_>>();
    shop_identifiers.push(record_with_shop_id_pk.shop_id.into());

    let _ = repository
        .put_shop_records_transact(existing_shop_records.clone())
        .await
        .unwrap();

    let mut new_shop = shop.clone();
    new_shop.name = "Hans' Shop".into();
    new_shop.domains = [Domain::try_from("https://foo.fr").unwrap()].into();
    let put = vec![
        ShopRecord::clone_from_shop_as_shop_domain_records(&new_shop)
            .first()
            .unwrap()
            .clone(),
    ];
    let update_record = ShopRecordUpdate {
        name: Some("Hans' Shop".into()),
        shop_type: Faker.fake(),
        domains: Some(
            [
                Domain::try_from("https://foo.com").unwrap(),
                Domain::try_from("https://foo.fr").unwrap(),
            ]
            .into(),
        ),
        image: Some(Url::parse("https://foo.bar").unwrap()),
        updated: OffsetDateTime::now_utc(),
    };
    let update = HashMap::from_iter([
        (
            ShopIdentifier::from(Domain::try_from("https://foo.com").unwrap()),
            update_record.clone(),
        ),
        (ShopIdentifier::from(shop.shop_id), update_record),
    ]);
    let delete = vec![
        ShopIdentifier::from(Domain::try_from("https://foo.de").unwrap()),
        ShopIdentifier::from(Domain::try_from("https://foo.en").unwrap()),
    ];
    let _ = repository
        .transact_write(put, update, delete)
        .await
        .unwrap();

    let actual_shop_id_record = repository
        .get_shop_record_by_id(&shop.shop_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(ShopName::from("Hans' Shop"), actual_shop_id_record.name);
    assert_eq!(
        Url::parse("https://foo.bar").unwrap(),
        actual_shop_id_record.image.unwrap()
    );
    assert_eq!(2, actual_shop_id_record.domains.len());
    assert!(actual_shop_id_record.domains.iter().all(|domain| domain
        == &Domain::try_from("https://foo.com").unwrap()
        || domain == &Domain::try_from("https://foo.fr").unwrap()));

    let actual_shop_url_record_de = repository
        .get_shop_record_by_domain(&Domain::try_from("https://foo.de").unwrap())
        .await
        .unwrap();
    assert!(actual_shop_url_record_de.is_none());

    let actual_shop_url_record_en = repository
        .get_shop_record_by_domain(&Domain::try_from("https://foo.en").unwrap())
        .await
        .unwrap();
    assert!(actual_shop_url_record_en.is_none());

    let actual_shop_url_record_com = repository
        .get_shop_record_by_domain(&Domain::try_from("https://foo.com").unwrap())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        ShopName::from("Hans' Shop"),
        actual_shop_url_record_com.name
    );
    assert_eq!(
        Url::parse("https://foo.bar").unwrap(),
        actual_shop_url_record_com.image.unwrap()
    );
    assert_eq!(2, actual_shop_url_record_com.domains.len());
    assert!(
        actual_shop_url_record_com
            .domains
            .iter()
            .all(
                |domain| domain == &Domain::try_from("https://foo.com").unwrap()
                    || domain == &Domain::try_from("https://foo.fr").unwrap()
            )
    );

    let actual_shop_url_record_fr = repository
        .get_shop_record_by_domain(&Domain::try_from("https://foo.fr").unwrap())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(ShopName::from("Hans' Shop"), actual_shop_url_record_fr.name);
}

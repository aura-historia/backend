use common::batch::Batch;
use common::domain::Domain;
use common::shop_id::ShopId;
use common::shop_name::ShopName;
use fake::{Fake, Faker};
use shop::core::shop::Shop;
use shop::dynamodb::raw_shop_name_record::RawShopNameRecord;
use shop::dynamodb::shop_record_update::ShopRecordUpdate;
use shop::dynamodb::shop_type_record::ShopTypeRecord;
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

    let actual = repository.get_shop_record(&Faker.fake()).await.unwrap();

    assert!(actual.is_none());
}

#[localstack_test(services = [DynamoDB()])]
async fn should_return_none_when_shop_record_not_exists_for_query_shop_id() {
    let repository = get_repository().await;

    let actual = repository.query_shop_id(&Faker.fake()).await.unwrap();

    assert!(actual.is_none());
}

#[localstack_test(services = [DynamoDB()])]
async fn should_return_some_when_shop_record_exists_for_get_by_id() {
    let repository = get_repository().await;

    let expected = ShopRecord::from(Faker.fake::<Shop>());
    let _ = repository.put_shop_record(expected.clone()).await.unwrap();
    let actual = repository
        .get_shop_record(&expected.shop_id)
        .await
        .unwrap()
        .unwrap();

    assert_eq!(expected, actual);
}

#[localstack_test(services = [DynamoDB()])]
async fn should_return_some_when_shop_record_exists_for_query_shop_id() {
    let repository = get_repository().await;

    let expected = ShopRecord::from(Faker.fake::<Shop>());
    let _ = repository.put_shop_record(expected.clone()).await.unwrap();
    let actual = repository
        .query_shop_id(&expected.shop_slug_id)
        .await
        .unwrap()
        .unwrap();

    assert_eq!(expected.shop_id, actual);
}

#[localstack_test(services = [DynamoDB()])]
async fn should_return_some_when_shop_record_exists_for_query_shop_by_shopify_domain() {
    let repository = get_repository().await;

    let shopify_domain = Domain::try_from("partner-shop.myshopify.com").unwrap();
    let mut expected = ShopRecord::from(Faker.fake::<Shop>());
    expected.shopify_domain = Some(shopify_domain.clone());
    expected.gsi3_pk = Some(shop::dynamodb::shop_record::mk_gsi3_pk(&shopify_domain));
    expected.gsi3_sk = Some(shop::dynamodb::shop_record::mk_gsi3_sk().to_owned());
    let _ = repository.put_shop_record(expected.clone()).await.unwrap();

    let actual = repository
        .query_shop_by_shopify_domain(&shopify_domain)
        .await
        .unwrap()
        .unwrap();

    assert_eq!(expected, actual);
}

#[localstack_test(services = [DynamoDB()])]
async fn should_return_none_when_shop_record_not_exists_for_update_shop_record() {
    let repository = get_repository().await;
    let update = ShopRecordUpdate {
        partner_user_id: None,
        gsi1_pk: None,
        gsi1_sk: None,
        gsi3_pk: None,
        gsi3_sk: None,
        shop_type: Some(ShopTypeRecord::Marketplace),
        domains: Some(HashSet::from([Domain::try_from("test-shop.com").unwrap()])),
        shopify_domain: None,
        shopify_currency: None,
        shopify_language: None,
        woocommerce_webhook_secret: None,
        woocommerce_currency: None,
        woocommerce_language: None,
        url: None,
        image: None,
        structured_address_addressline: None,
        structured_address_addressline_extra: None,
        structured_address_locality: None,
        structured_address_region: None,
        structured_address_postal_code: None,
        structured_address_country: None,
        geo_address_lat: None,
        geo_address_lon: None,
        phone: None,
        email: None,
        partner_api_key_short: None,
        partner_api_key_long_hash: None,
        updated: OffsetDateTime::now_utc(),
    };

    let actual = repository
        .update_shop_record(&ShopId::new(), update)
        .await
        .unwrap();

    assert!(actual.is_none());
}

#[localstack_test(services = [DynamoDB()])]
async fn should_return_updated_record_when_updating_all_fields_for_update_shop_record() {
    let repository = get_repository().await;
    let initial = ShopRecord::from(Faker.fake::<Shop>());
    let _ = repository.put_shop_record(initial.clone()).await.unwrap();

    let new_shop_type: ShopTypeRecord = Faker.fake();
    let new_domains = HashSet::from([
        Domain::try_from("updated-shop.com").unwrap(),
        Domain::try_from("updated-shop.de").unwrap(),
    ]);
    let new_image = Url::parse("https://updated-shop.com/banner.png").unwrap();
    let updated_at = OffsetDateTime::now_utc();

    let update = ShopRecordUpdate {
        partner_user_id: None,
        gsi1_pk: None,
        gsi1_sk: None,
        gsi3_pk: None,
        gsi3_sk: None,
        shop_type: Some(new_shop_type),
        domains: Some(new_domains.clone()),
        shopify_domain: None,
        shopify_currency: None,
        shopify_language: None,
        woocommerce_webhook_secret: None,
        woocommerce_currency: None,
        woocommerce_language: None,
        url: None,
        image: Some(new_image.clone()),
        structured_address_addressline: None,
        structured_address_addressline_extra: None,
        structured_address_locality: None,
        structured_address_region: None,
        structured_address_postal_code: None,
        structured_address_country: None,
        geo_address_lat: None,
        geo_address_lon: None,
        phone: None,
        email: None,
        partner_api_key_short: None,
        partner_api_key_long_hash: None,
        updated: updated_at,
    };

    let result = repository
        .update_shop_record(&initial.shop_id, update)
        .await
        .unwrap()
        .unwrap();

    assert_eq!(result.shop_id, initial.shop_id);
    assert_eq!(result.shop_slug_id, initial.shop_slug_id);
    assert_eq!(result.name, initial.name);
    assert_eq!(result.created, initial.created);
    assert_eq!(result.shop_type, new_shop_type);
    assert_eq!(result.domains, new_domains);
    assert_eq!(result.image, Some(new_image));
    assert_eq!(result.updated, updated_at);
}

#[localstack_test(services = [DynamoDB()])]
async fn should_preserve_unchanged_fields_when_updating_only_timestamp_for_update_shop_record() {
    let repository = get_repository().await;
    let initial = ShopRecord::from(Faker.fake::<Shop>());
    let _ = repository.put_shop_record(initial.clone()).await.unwrap();

    let updated_at = OffsetDateTime::now_utc();
    let update = ShopRecordUpdate {
        partner_user_id: None,
        gsi1_pk: None,
        gsi1_sk: None,
        gsi3_pk: None,
        gsi3_sk: None,
        shop_type: None,
        domains: None,
        shopify_domain: None,
        shopify_currency: None,
        shopify_language: None,
        woocommerce_webhook_secret: None,
        woocommerce_currency: None,
        woocommerce_language: None,
        url: None,
        image: None,
        structured_address_addressline: None,
        structured_address_addressline_extra: None,
        structured_address_locality: None,
        structured_address_region: None,
        structured_address_postal_code: None,
        structured_address_country: None,
        geo_address_lat: None,
        geo_address_lon: None,
        phone: None,
        email: None,
        partner_api_key_short: None,
        partner_api_key_long_hash: None,
        updated: updated_at,
    };

    let result = repository
        .update_shop_record(&initial.shop_id, update)
        .await
        .unwrap()
        .unwrap();

    assert_eq!(result.shop_id, initial.shop_id);
    assert_eq!(result.shop_slug_id, initial.shop_slug_id);
    assert_eq!(result.name, initial.name);
    assert_eq!(result.shop_type, initial.shop_type);
    assert_eq!(result.domains, initial.domains);
    assert_eq!(result.image, initial.image);
    assert_eq!(result.created, initial.created);
    assert_eq!(result.updated, updated_at);
}

#[localstack_test(services = [DynamoDB()])]
async fn should_return_all_records_when_all_exist_for_get_shop_records() {
    let repository = get_repository().await;
    let record1 = ShopRecord::from(Faker.fake::<Shop>());
    let record2 = ShopRecord::from(Faker.fake::<Shop>());
    let record3 = ShopRecord::from(Faker.fake::<Shop>());

    for record in [&record1, &record2, &record3] {
        repository.put_shop_record(record.clone()).await.unwrap();
    }

    let batch = Batch::from([record1.shop_id, record2.shop_id, record3.shop_id]);
    let mut result = repository.get_shop_records(&batch).await.unwrap();

    assert!(result.unprocessed.is_none());
    assert_eq!(result.items.len(), 3);

    let mut expected = vec![record1, record2, record3];
    expected.sort_by_key(|r| r.shop_id);
    result.items.sort_by_key(|r| r.shop_id);
    assert_eq!(result.items, expected);
}

#[localstack_test(services = [DynamoDB()])]
async fn should_return_only_existing_records_when_some_do_not_exist_for_get_shop_records() {
    let repository = get_repository().await;
    let record1 = ShopRecord::from(Faker.fake::<Shop>());
    let record2 = ShopRecord::from(Faker.fake::<Shop>());
    let record3 = ShopRecord::from(Faker.fake::<Shop>());

    for record in [&record1, &record2, &record3] {
        repository.put_shop_record(record.clone()).await.unwrap();
    }

    let batch = Batch::try_from(vec![
        record1.shop_id,
        record2.shop_id,
        record3.shop_id,
        ShopId::new(),
        ShopId::new(),
    ])
    .unwrap();
    let mut result = repository.get_shop_records(&batch).await.unwrap();

    assert!(result.unprocessed.is_none());
    assert_eq!(result.items.len(), 3);

    let mut expected = vec![record1, record2, record3];
    expected.sort_by_key(|r| r.shop_id);
    result.items.sort_by_key(|r| r.shop_id);
    assert_eq!(result.items, expected);
}

#[localstack_test(services = [DynamoDB()])]
async fn should_return_none_when_raw_shop_name_record_not_exists_for_get_raw_shop_name_record() {
    let repository = get_repository().await;

    let actual = repository
        .get_raw_shop_name_record(&Faker.fake::<ShopName>())
        .await
        .unwrap();

    assert!(actual.is_none());
}

#[localstack_test(services = [DynamoDB()])]
async fn should_return_some_when_raw_shop_name_record_exists_for_get_raw_shop_name_record() {
    let repository = get_repository().await;

    let expected: RawShopNameRecord = Faker.fake();
    repository
        .put_raw_shop_name_record(expected.clone())
        .await
        .unwrap();
    let actual = repository
        .get_raw_shop_name_record(&expected.raw_name)
        .await
        .unwrap()
        .unwrap();

    assert_eq!(expected, actual);
}

#[localstack_test(services = [DynamoDB()])]
async fn should_return_none_when_different_raw_shop_name_not_exists_for_get_raw_shop_name_record() {
    let repository = get_repository().await;

    let record: RawShopNameRecord = Faker.fake();
    repository.put_raw_shop_name_record(record).await.unwrap();
    let actual = repository
        .get_raw_shop_name_record(&Faker.fake::<ShopName>())
        .await
        .unwrap();

    assert!(actual.is_none());
}

#[localstack_test(services = [DynamoDB()])]
async fn should_return_shops_for_partner_when_exists_for_query_shops_by_partner() {
    let repository = get_repository().await;
    let partner_user_id = common::user_id::UserId::new();

    let mut shop_record: ShopRecord = Faker.fake();
    shop_record.partner_user_id = Some(partner_user_id);
    shop_record.gsi1_pk = Some(shop::dynamodb::shop_record::mk_gsi1_pk(&partner_user_id));
    shop_record.gsi1_sk = Some(shop::dynamodb::shop_record::mk_gsi1_sk(
        &shop_record.shop_id,
    ));
    repository
        .put_shop_record(shop_record.clone())
        .await
        .unwrap();

    let actual = repository
        .query_shops_by_partner(&partner_user_id)
        .await
        .unwrap();

    assert_eq!(1, actual.len());
    assert_eq!(shop_record.shop_id, actual[0].shop_id);
}

#[localstack_test(services = [DynamoDB()])]
async fn should_return_empty_when_no_shops_for_partner_for_query_shops_by_partner() {
    let repository = get_repository().await;
    let partner_user_id = common::user_id::UserId::new();

    let actual = repository
        .query_shops_by_partner(&partner_user_id)
        .await
        .unwrap();

    assert!(actual.is_empty());
}

#[localstack_test(services = [DynamoDB()])]
async fn should_return_multiple_shops_for_partner_for_query_shops_by_partner() {
    let repository = get_repository().await;
    let partner_user_id = common::user_id::UserId::new();

    for _ in 0..3 {
        let mut shop_record: ShopRecord = Faker.fake();
        shop_record.partner_user_id = Some(partner_user_id);
        shop_record.gsi1_pk = Some(shop::dynamodb::shop_record::mk_gsi1_pk(&partner_user_id));
        shop_record.gsi1_sk = Some(shop::dynamodb::shop_record::mk_gsi1_sk(
            &shop_record.shop_id,
        ));
        repository.put_shop_record(shop_record).await.unwrap();
    }

    let actual = repository
        .query_shops_by_partner(&partner_user_id)
        .await
        .unwrap();

    assert_eq!(3, actual.len());
}

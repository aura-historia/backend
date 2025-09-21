use fake::{Fake, Faker};
use shop_dynamodb::{
    repository::{ShopDynamoDbRepository, ShopDynamoDbRepositoryImpl},
    shop_record::ShopRecord,
};
use test_api::*;

async fn get_repository() -> ShopDynamoDbRepositoryImpl<'static> {
    ShopDynamoDbRepositoryImpl::new(get_dynamodb_client().await, "table_1")
}

#[localstack_test(services = [DynamoDB()])]
async fn should_return_none_when_shop_record_not_exists() {
    let repository = get_repository().await;

    let actual = repository.get_shop_record(&Faker.fake()).await.unwrap();

    assert!(actual.is_none());
}

#[localstack_test(services = [DynamoDB()])]
async fn should_return_some_when_shop_record_exists() {
    let repository = get_repository().await;

    let expected = Faker.fake::<ShopRecord>();
    let _ = repository.put_shop_record(expected.clone()).await.unwrap();
    let actual = repository
        .get_shop_record(&expected.shop_id)
        .await
        .unwrap()
        .unwrap();

    assert_eq!(expected, actual);
}

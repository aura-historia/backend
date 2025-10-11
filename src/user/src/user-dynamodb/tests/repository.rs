use fake::{Fake, Faker};
use test_api::*;
use user_dynamodb::{
    repository::{UserDynamoDbRepository, UserDynamoDbRepositoryImpl},
    user_record::UserRecord,
};

#[localstack_test(services = [DynamoDB()])]
async fn should_return_none_when_not_exists() {
    let repository = UserDynamoDbRepositoryImpl::new(get_dynamodb_client().await, "table_1");

    let actual = repository.get_user_record(&Faker.fake()).await.unwrap();

    assert!(actual.is_none());
}

#[localstack_test(services = [DynamoDB()])]
async fn should_return_some_when_exists() {
    let repository = UserDynamoDbRepositoryImpl::new(get_dynamodb_client().await, "table_1");
    let expected = Faker.fake::<UserRecord>();

    let _ = repository.put_user_record(expected.clone()).await.unwrap();

    let actual = repository
        .get_user_record(&expected.id)
        .await
        .unwrap()
        .unwrap();

    assert_eq!(expected, actual);
}

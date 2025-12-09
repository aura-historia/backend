use fake::{Fake, Faker};
use test_api::*;
use user::dynamodb::{
    repository::{UserDynamoDbRepository, UserDynamoDbRepositoryImpl},
    user_record::UserRecord,
    user_record_update::UserRecordUpdate,
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

#[rstest::rstest]
#[test_attr(apply(test))]
#[case(Faker.fake())]
#[case(Faker.fake())]
#[case(Faker.fake())]
#[case(Faker.fake())]
#[case(Faker.fake())]
#[case(Faker.fake())]
#[case(Faker.fake())]
#[case(Faker.fake())]
#[case(Faker.fake())]
#[case(Faker.fake())]
#[localstack_test(services = [DynamoDB()])]
async fn should_update_user_record(#[case] user_record_update: UserRecordUpdate) {
    let repository = UserDynamoDbRepositoryImpl::new(get_dynamodb_client().await, "table_1");
    let initial = Faker.fake::<UserRecord>();

    let _ = repository.put_user_record(initial.clone()).await.unwrap();

    let updated = repository
        .update_user_record(&initial.id, user_record_update.clone())
        .await
        .unwrap()
        .unwrap();
    let actual = repository
        .get_user_record(&initial.id)
        .await
        .unwrap()
        .unwrap();

    if let Some(email) = user_record_update.email {
        assert_eq!(email, updated.email);
    }
    if let Some(ref first_name) = user_record_update.first_name {
        assert_eq!(first_name, updated.first_name.as_ref().unwrap());
    }
    if let Some(ref last_name) = user_record_update.last_name {
        assert_eq!(last_name, updated.last_name.as_ref().unwrap());
    }
    if let Some(ref language) = user_record_update.language {
        assert_eq!(language, updated.language.as_ref().unwrap());
    }
    if let Some(ref currency) = user_record_update.currency {
        assert_eq!(currency, updated.currency.as_ref().unwrap());
    }

    assert_eq!(updated, actual);
}

use common::user_id::UserId;
use fake::{Fake, Faker};
use search_filter::dynamodb::repository::{
    UserSearchFilterDynamoDbRepository, UserSearchFilterDynamoDbRepositoryImpl,
};
use search_filter::dynamodb::user_search_filter_record::{UserSearchFilterRecord, mk_pk};
use test_api::*;

async fn get_repository() -> UserSearchFilterDynamoDbRepositoryImpl<'static> {
    UserSearchFilterDynamoDbRepositoryImpl::new(get_dynamodb_client().await, "table_1")
}

#[localstack_test(services = [DynamoDB()])]
async fn should_return_none_when_table_empty() {
    let actual = get_repository()
        .await
        .get_user_search_filter_record(&Faker.fake(), &Faker.fake())
        .await
        .unwrap();

    assert!(actual.is_none())
}

#[localstack_test(services = [DynamoDB()])]
async fn should_return_none_when_partition_empty() {
    let repository = get_repository().await;
    for record in fake::vec![UserSearchFilterRecord; 42] {
        let _ = repository
            .put_user_search_filter_record(record)
            .await
            .unwrap();
    }

    let actual = repository
        .get_user_search_filter_record(&Faker.fake(), &Faker.fake())
        .await
        .unwrap();

    assert!(actual.is_none())
}

#[localstack_test(services = [DynamoDB()])]
async fn should_return_none_when_partition_non_empty_but_filter_id_not_exists() {
    let repository = get_repository().await;
    let user_id = UserId::new();
    let records = fake::vec![UserSearchFilterRecord; 37];
    for mut record in records {
        record.pk = mk_pk(&user_id);
        record.user_id = user_id;
        let _ = repository
            .put_user_search_filter_record(record)
            .await
            .unwrap();
    }

    let actual = repository
        .get_user_search_filter_record(&user_id, &Faker.fake())
        .await
        .unwrap();

    assert!(actual.is_none())
}

#[localstack_test(services = [DynamoDB()])]
async fn should_return_some_when_partition_non_empty_and_filter_id_exists() {
    let repository = get_repository().await;
    let user_id = UserId::new();
    let records = fake::vec![UserSearchFilterRecord; 37];
    for mut record in records {
        record.pk = mk_pk(&user_id);
        record.user_id = user_id;
        let _ = repository
            .put_user_search_filter_record(record)
            .await
            .unwrap();
    }
    let mut expected = Faker.fake::<UserSearchFilterRecord>();
    expected.pk = mk_pk(&user_id);
    expected.user_id = user_id;
    let _ = repository
        .put_user_search_filter_record(expected.clone())
        .await
        .unwrap();

    let actual = repository
        .get_user_search_filter_record(&user_id, &expected.user_search_filter_id)
        .await
        .unwrap()
        .unwrap();

    assert_eq!(expected, actual);
}

#[localstack_test(services = [DynamoDB()])]
async fn should_return_embedding_when_record_has_embedding() {
    let repository = get_repository().await;
    let mut expected = Faker.fake::<UserSearchFilterRecord>();
    expected.embedding = Some(vec![0.1, 0.2, 0.3]);
    repository
        .put_user_search_filter_record(expected.clone())
        .await
        .unwrap();

    let actual = repository
        .get_user_search_filter_record(&expected.user_id, &expected.user_search_filter_id)
        .await
        .unwrap()
        .unwrap();

    assert_eq!(expected, actual);
}

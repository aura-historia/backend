use common::user_id::UserId;
use fake::{Fake, Faker};
use search_filter_dynamodb::repository::{
    SearchFilterDynamoDbRepository, SearchFilterDynamoDbRepositoryImpl,
};
use search_filter_dynamodb::search_filter_record::{SearchFilterRecord, mk_pk};
use test_api::*;

async fn get_repository() -> SearchFilterDynamoDbRepositoryImpl<'static> {
    SearchFilterDynamoDbRepositoryImpl::new(get_dynamodb_client().await, "table_1")
}

#[localstack_test(services = [DynamoDB()])]
async fn should_succeed_when_table_empty() {
    let _ = get_repository()
        .await
        .delete_search_filter_record(&Faker.fake(), &Faker.fake())
        .await
        .unwrap();
}

#[localstack_test(services = [DynamoDB()])]
async fn should_succeed_when_partition_empty() {
    let repository = get_repository().await;
    for record in fake::vec![SearchFilterRecord; 42] {
        let _ = repository.put_search_filter_record(record).await.unwrap();
    }

    let _ = repository
        .delete_search_filter_record(&Faker.fake(), &Faker.fake())
        .await
        .unwrap();

    let remaining = get_dynamodb_client()
        .await
        .scan()
        .table_name("table_1")
        .send()
        .await
        .unwrap()
        .count;
    assert_eq!(42, remaining);
}

#[localstack_test(services = [DynamoDB()])]
async fn should_succeed_when_partition_non_empty_but_filter_id_not_exists() {
    let repository = get_repository().await;
    let user_id = UserId::new();
    let records = fake::vec![SearchFilterRecord; 37];
    for mut record in records {
        record.pk = mk_pk(&user_id);
        record.user_id = user_id;
        let _ = repository.put_search_filter_record(record).await.unwrap();
    }

    let _ = repository
        .delete_search_filter_record(&user_id, &Faker.fake())
        .await
        .unwrap();

    let remaining = get_dynamodb_client()
        .await
        .scan()
        .table_name("table_1")
        .send()
        .await
        .unwrap()
        .count;
    assert_eq!(37, remaining);
}

#[localstack_test(services = [DynamoDB()])]
async fn should_delete_when_partition_non_empty_and_filter_id_exists() {
    let repository = get_repository().await;
    let user_id = UserId::new();
    let records = fake::vec![SearchFilterRecord; 37];
    for mut record in records {
        record.pk = mk_pk(&user_id);
        record.user_id = user_id;
        let _ = repository.put_search_filter_record(record).await.unwrap();
    }
    let mut expected = Faker.fake::<SearchFilterRecord>();
    expected.pk = mk_pk(&user_id);
    expected.user_id = user_id;
    let _ = repository
        .put_search_filter_record(expected.clone())
        .await
        .unwrap();

    let _ = repository
        .delete_search_filter_record(&user_id, &expected.search_filter_id)
        .await
        .unwrap();

    let actual = repository
        .get_search_filter_record(&user_id, &expected.search_filter_id)
        .await
        .unwrap();

    assert!(actual.is_none());
    let remaining = get_dynamodb_client()
        .await
        .scan()
        .table_name("table_1")
        .send()
        .await
        .unwrap()
        .count;
    assert_eq!(37, remaining);
}

use common::user_id::UserId;
use fake::{Fake, Faker};
use search_filter::dynamodb::repository::{
    UserSearchFilterDynamoDbRepository, UserSearchFilterDynamoDbRepositoryImpl,
};
use search_filter::dynamodb::user_search_filter_record::UserSearchFilterRecord;
use search_filter::dynamodb::user_search_filter_record::mk_pk;
use test_api::*;

async fn get_repository() -> UserSearchFilterDynamoDbRepositoryImpl<'static> {
    UserSearchFilterDynamoDbRepositoryImpl::new(get_dynamodb_client().await, "table_1")
}

#[rstest::rstest]
#[trace]
#[test_attr(apply(test))]
#[case::scan_index_forward_false(false)]
#[case::scan_index_forward_true(true)]
#[localstack_test(services = [DynamoDB()])]
async fn should_return_no_records_when_partition_empty(#[case] scan_index_forward: bool) {
    let actual = get_repository()
        .await
        .query_user_search_filter_records(&Faker.fake(), scan_index_forward)
        .await
        .unwrap();

    assert!(actual.is_empty())
}

#[rstest::rstest]
#[trace]
#[test_attr(apply(test))]
#[case::scan_index_forward_false(false)]
#[case::scan_index_forward_true(true)]
#[localstack_test(services = [DynamoDB()])]
async fn should_return_records_only_for_target_user(#[case] scan_index_forward: bool) {
    let repository = get_repository().await;
    let expected = Faker.fake::<UserSearchFilterRecord>();
    let _ = repository
        .put_user_search_filter_record(expected.clone())
        .await
        .unwrap();
    let other = Faker.fake::<UserSearchFilterRecord>();
    let _ = repository
        .put_user_search_filter_record(other.clone())
        .await
        .unwrap();

    let actual = repository
        .query_user_search_filter_records(&expected.user_id, scan_index_forward)
        .await
        .unwrap();

    assert_eq!(vec![expected], actual);
}

#[localstack_test(services = [DynamoDB()])]
async fn should_sort_by_time_aka_uuidv7_asc_when_scan_index_forward_true() {
    let repository = get_repository().await;
    let user_id = UserId::new();
    let mut records = Vec::with_capacity(100);
    for _ in 0..100 {
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        let mut record = Faker.fake::<UserSearchFilterRecord>();
        record.pk = mk_pk(&user_id);
        record.user_id = user_id;
        let _ = repository
            .put_user_search_filter_record(record.clone())
            .await
            .unwrap();
        records.push(record);
    }

    records.sort_by_key(|l| l.created);

    let actual = repository
        .query_user_search_filter_records(&user_id, true)
        .await
        .unwrap();

    assert_eq!(records, actual);
}

#[localstack_test(services = [DynamoDB()])]
async fn should_sort_by_time_aka_uuidv7_desc_when_scan_index_forward_false() {
    let repository = get_repository().await;
    let user_id = UserId::new();
    let mut records = Vec::with_capacity(100);
    for _ in 0..100 {
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        let mut record = Faker.fake::<UserSearchFilterRecord>();
        record.pk = mk_pk(&user_id);
        record.user_id = user_id;
        let _ = repository
            .put_user_search_filter_record(record.clone())
            .await
            .unwrap();
        records.push(record);
    }

    records.sort_by(|l, r| l.created.cmp(&r.created).reverse());

    let actual = repository
        .query_user_search_filter_records(&user_id, false)
        .await
        .unwrap();

    assert_eq!(records, actual);
}

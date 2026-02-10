use fake::{Fake, Faker};
use product_classification::category::{
    dynamodb_repository::{CategoryDynamoDbRepository, CategoryDynamoDbRepositoryImpl},
    record::CategoryRecord,
};
use test_api::*;

#[localstack_test(services = [DynamoDB()])]
async fn should_return_none_when_record_not_exists() {
    let repository = CategoryDynamoDbRepositoryImpl::new(get_dynamodb_client().await, "table_1");
    let res = repository.get_category_record(&Faker.fake()).await.unwrap();

    assert!(res.is_none());
}

#[localstack_test(services = [DynamoDB()])]
async fn should_put_then_get_category_record() {
    let repository = CategoryDynamoDbRepositoryImpl::new(get_dynamodb_client().await, "table_1");

    let record = Faker.fake::<CategoryRecord>();
    let _ = repository
        .put_category_record(record.clone())
        .await
        .unwrap();
    let res = repository
        .get_category_record(&record.category_id)
        .await
        .unwrap()
        .unwrap();

    assert_eq!(record, res);
}

#[localstack_test(services = [DynamoDB()])]
async fn should_put_then_query_category_records() {
    let repository = CategoryDynamoDbRepositoryImpl::new(get_dynamodb_client().await, "table_1");

    let records = fake::vec![CategoryRecord; 10];
    for record in &records {
        let _ = repository
            .put_category_record(record.clone())
            .await
            .unwrap();
    }

    let retrieved_all = repository
        .query_category_records()
        .await
        .unwrap()
        .into_iter()
        .all(|record| records.contains(&record));
    assert!(retrieved_all);
}

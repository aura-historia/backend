use common::batch::Batch;
use fake::{Fake, Faker};
use product::dynamodb::product_event_record::ProductEventRecord;
use product::dynamodb::product_event_record::domain::ProductDomainEventRecord;
use product::dynamodb::product_meta_record::ProductMetaRecord;
use product::dynamodb::product_record::ProductRecord;
use product::dynamodb::repository::{ProductDynamoDbRepository, ProductDynamoDbRepositoryImpl};
use product::dynamodb::test_utils::{
    product_record_to_created_event_record, product_record_to_meta_record,
};
use test_api::*;

async fn get_repository() -> ProductDynamoDbRepositoryImpl<'static> {
    ProductDynamoDbRepositoryImpl::new(get_dynamodb_client().await, "table_1")
}

#[rstest::rstest]
#[test_attr(apply(test))]
#[case(ProductEventRecord::Domain(Faker.fake()))]
#[case(ProductEventRecord::Enrichment(Faker.fake()))]
#[case(ProductEventRecord::Policy(Faker.fake()))]
#[trace]
#[localstack_test(services = [DynamoDB()])]
async fn should_put_product_event_records_for_single_record(#[case] expected: ProductEventRecord) {
    get_repository()
        .await
        .put_product_event_records(Batch::from([expected.clone()]))
        .await
        .unwrap();

    let actual = get_dynamodb_client()
        .await
        .scan()
        .table_name("table_1")
        .send()
        .await
        .unwrap()
        .items
        .unwrap()
        .into_iter()
        .map(serde_dynamo::from_item)
        .collect::<Result<Vec<ProductEventRecord>, _>>()
        .unwrap();

    assert_eq!(vec![expected], actual);
}

#[localstack_test(services = [DynamoDB()])]
async fn should_put_product_event_records_for_multiple_records() {
    let expected1 = Faker.fake::<ProductDomainEventRecord>();
    let expected2 = Faker.fake::<ProductDomainEventRecord>();

    get_repository()
        .await
        .put_product_event_records(Batch::from([
            expected1.clone().into(),
            expected2.clone().into(),
        ]))
        .await
        .unwrap();

    let actual = get_dynamodb_client()
        .await
        .scan()
        .table_name("table_1")
        .send()
        .await
        .unwrap()
        .items
        .unwrap()
        .into_iter()
        .map(serde_dynamo::from_item)
        .collect::<Result<Vec<ProductDomainEventRecord>, _>>()
        .unwrap();

    assert_eq!(vec![expected1, expected2], actual);
}

#[localstack_test(services = [DynamoDB()])]
async fn should_transact_write_product_event_records_with_meta_record() {
    let repository = get_repository().await;
    let product_record = Faker.fake::<ProductRecord>();
    let event_record = product_record_to_created_event_record(&product_record);
    let meta_record = product_record_to_meta_record(&product_record, 1);

    repository
        .transact_write_product_event_records(
            vec![ProductEventRecord::Domain(event_record.clone())],
            meta_record.clone(),
            0,
        )
        .await
        .unwrap();

    let items = get_dynamodb_client()
        .await
        .scan()
        .table_name("table_1")
        .send()
        .await
        .unwrap()
        .items
        .unwrap();
    assert_eq!(2, items.len());
    assert!(items.iter().any(|item| {
        serde_dynamo::from_item::<_, ProductDomainEventRecord>(item.clone())
            == Ok(event_record.clone())
    }));
    assert!(items.iter().any(|item| {
        serde_dynamo::from_item::<_, ProductMetaRecord>(item.clone()) == Ok(meta_record.clone())
    }));
}

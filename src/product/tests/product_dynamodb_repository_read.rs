use common::event_id::EventId;
use fake::{Fake, Faker};
use product::dynamodb::product_event_record::ProductEventRecord;
use product::dynamodb::product_event_record::domain::ProductDomainEventRecord;
use product::dynamodb::product_event_type_record::domain::ProductDomainEventTypeRecord;
use product::dynamodb::product_record::ProductRecord;
use product::dynamodb::repository::{ProductDynamoDbRepository, ProductDynamoDbRepositoryImpl};
use product::dynamodb::test_utils::{
    product_record_to_created_event_record, product_record_to_meta_record,
};
use test_api::*;

async fn get_repository() -> ProductDynamoDbRepositoryImpl<'static> {
    ProductDynamoDbRepositoryImpl::new(get_dynamodb_client().await, "table_1")
}

#[localstack_test(services = [DynamoDB()])]
async fn should_return_empty_events_when_partition_empty() {
    let repository = get_repository().await;

    let actual = repository
        .query_product_event_records(&Faker.fake(), &Faker.fake())
        .await
        .unwrap();

    assert!(actual.is_empty());
}

#[localstack_test(services = [DynamoDB()])]
async fn should_return_product_event_records_in_sort_key_order() {
    let repository = get_repository().await;
    let product_record = Faker.fake::<ProductRecord>();
    let mut created_event = product_record_to_created_event_record(&product_record);
    created_event.event_id = EventId::new();
    created_event.sk =
        product::dynamodb::product_event_record::domain::mk_sk(&created_event.event_id);
    let mut updated_event = product_record_to_created_event_record(&product_record);
    updated_event.event_id = EventId::new();
    updated_event.sk =
        product::dynamodb::product_event_record::domain::mk_sk(&updated_event.event_id);
    updated_event.event_type = ProductDomainEventTypeRecord::DomainStateChanged;
    updated_event.old_state = Some(Faker.fake());

    repository
        .transact_write_product_event_records(
            vec![
                ProductEventRecord::Domain(created_event.clone()),
                ProductEventRecord::Domain(updated_event.clone()),
            ],
            product_record_to_meta_record(&product_record, 2),
            0,
        )
        .await
        .unwrap();

    let actual = repository
        .query_product_domain_event_records(
            &product_record.shop_id,
            &product_record.shops_product_id,
        )
        .await
        .unwrap();

    assert_eq!(vec![created_event, updated_event], actual);
}

#[localstack_test(services = [DynamoDB()])]
async fn should_return_product_id_from_meta_record() {
    let repository = get_repository().await;
    let product_record = Faker.fake::<ProductRecord>();

    repository
        .transact_write_product_event_records(
            vec![ProductEventRecord::Domain(
                product_record_to_created_event_record(&product_record),
            )],
            product_record_to_meta_record(&product_record, 1),
            0,
        )
        .await
        .unwrap();

    let actual = repository
        .get_product_id(&product_record.shop_id, &product_record.shops_product_id)
        .await
        .unwrap();

    assert_eq!(Some(product_record.product_id), actual);
}

#[localstack_test(services = [DynamoDB()])]
async fn should_return_product_key_from_meta_record_gsi() {
    let repository = get_repository().await;
    let product_record = Faker.fake::<ProductRecord>();

    repository
        .transact_write_product_event_records(
            vec![ProductEventRecord::Domain(
                product_record_to_created_event_record(&product_record),
            )],
            product_record_to_meta_record(&product_record, 1),
            0,
        )
        .await
        .unwrap();

    let actual = repository
        .query_product_key(
            &product_record.shop_slug_id,
            &product_record.product_slug_id,
        )
        .await
        .unwrap()
        .unwrap();

    assert_eq!(product_record.shop_id, actual.shop_id);
    assert_eq!(product_record.shops_product_id, actual.shops_product_id);
}

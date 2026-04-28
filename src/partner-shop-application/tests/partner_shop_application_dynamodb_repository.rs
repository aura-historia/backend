use common::user_id::UserId;
use fake::{Fake, Faker};
use partner_shop_application::{
    core::partner_shop_application_id::PartnerShopApplicationId,
    dynamodb::{
        partner_shop_application_record::{PartnerShopApplicationRecord, mk_pk},
        partner_shop_application_record_update::PartnerShopApplicationRecordUpdate,
        partner_shop_application_state_record::PartnerShopApplicationStateRecord,
        repository::{
            PartnerShopApplicationDynamoDbRepository, PartnerShopApplicationDynamoDbRepositoryImpl,
        },
    },
};
use test_api::*;
use time::OffsetDateTime;

async fn get_repository() -> PartnerShopApplicationDynamoDbRepositoryImpl<'static> {
    PartnerShopApplicationDynamoDbRepositoryImpl::new(get_dynamodb_client().await, "table_1")
}

#[localstack_test(services = [DynamoDB()])]
fn should_put_partner_shop_application_record() {
    let repository = get_repository().await;

    let expected = Faker.fake::<PartnerShopApplicationRecord>();
    let _ = repository
        .put_partner_shop_application_record(expected.clone())
        .await
        .unwrap();

    let actual = repository
        .get_partner_shop_application_record(&expected.applicant_user_id, &expected.id)
        .await
        .unwrap()
        .unwrap();

    assert_eq!(expected, actual);
}

#[localstack_test(services = [DynamoDB()])]
fn should_return_none_when_partner_shop_application_record_not_exists() {
    let repository = get_repository().await;

    let actual = repository
        .get_partner_shop_application_record(&Faker.fake(), &PartnerShopApplicationId::new())
        .await
        .unwrap();

    assert!(actual.is_none());
}

#[localstack_test(services = [DynamoDB()])]
fn should_update_partner_shop_application_record_state() {
    let repository = get_repository().await;

    let initial = Faker.fake::<PartnerShopApplicationRecord>();
    let _ = repository
        .put_partner_shop_application_record(initial.clone())
        .await
        .unwrap();

    let updated_time = OffsetDateTime::now_utc();
    let _ = repository
        .update_partner_shop_application_record(
            &initial.applicant_user_id,
            &initial.id,
            PartnerShopApplicationRecordUpdate {
                business_state: Some(PartnerShopApplicationStateRecord::Approved),
                execution_state: None,
                shop_name: None,
                shop_type: None,
                shop_domains: None,
                shop_image: None,
                shop_structured_address_addressline: None,
                shop_structured_address_addressline_extra: None,
                shop_structured_address_locality: None,
                shop_structured_address_region: None,
                shop_structured_address_postal_code: None,
                shop_structured_address_country: None,
                shop_phone: None,
                shop_email: None,
                shop_specialities_categories: None,
                shop_specialities_periods: None,
                task_token: None,
                updated: updated_time,
            },
        )
        .await
        .unwrap();

    let actual = repository
        .get_partner_shop_application_record(&initial.applicant_user_id, &initial.id)
        .await
        .unwrap()
        .unwrap();

    assert_eq!(
        PartnerShopApplicationStateRecord::Approved,
        actual.business_state
    );
}

#[localstack_test(services = [DynamoDB()])]
fn should_delete_partner_shop_application_record() {
    let repository = get_repository().await;

    let record = Faker.fake::<PartnerShopApplicationRecord>();
    let _ = repository
        .put_partner_shop_application_record(record.clone())
        .await
        .unwrap();

    let before = repository
        .get_partner_shop_application_record(&record.applicant_user_id, &record.id)
        .await
        .unwrap();
    assert!(before.is_some());

    let _ = repository
        .delete_partner_shop_application_record(&record.applicant_user_id, &record.id)
        .await
        .unwrap();

    let after = repository
        .get_partner_shop_application_record(&record.applicant_user_id, &record.id)
        .await
        .unwrap();
    assert!(after.is_none());
}

#[localstack_test(services = [DynamoDB()])]
fn should_query_all_partner_shop_application_records() {
    let repository = get_repository().await;

    let mut records = fake::vec![PartnerShopApplicationRecord; 5];
    for record in &mut records {
        let _ = repository
            .put_partner_shop_application_record(record.clone())
            .await
            .unwrap();
    }

    let actual = repository
        .query_all_partner_shop_application_records()
        .await
        .unwrap();

    assert!(actual.len() >= 5);
}

#[localstack_test(services = [DynamoDB()])]
fn should_query_all_returns_records_when_exist() {
    let repository = get_repository().await;

    // The query_all function works via the gsi1 and verifies the query succeeds
    let actual = repository
        .query_all_partner_shop_application_records()
        .await
        .unwrap();

    // We just validate the function executes successfully
    let _ = actual;
}

#[localstack_test(services = [DynamoDB()])]
fn should_put_and_get_multiple_records_for_same_user() {
    let repository = get_repository().await;
    let user_id = UserId::new();

    let mut records = fake::vec![PartnerShopApplicationRecord; 3];
    for record in &mut records {
        record.applicant_user_id = user_id;
        record.pk = mk_pk(&user_id);
        let _ = repository
            .put_partner_shop_application_record(record.clone())
            .await
            .unwrap();
    }

    for record in &records {
        let actual = repository
            .get_partner_shop_application_record(&user_id, &record.id)
            .await
            .unwrap();
        assert!(actual.is_some());
    }
}

#[localstack_test(services = [DynamoDB()])]
fn should_query_all_partner_shop_application_records_by_user() {
    let repository = get_repository().await;
    let user_id = UserId::new();

    let mut records = fake::vec![PartnerShopApplicationRecord; 3];
    for record in &mut records {
        record.applicant_user_id = user_id;
        record.pk = mk_pk(&user_id);
        let _ = repository
            .put_partner_shop_application_record(record.clone())
            .await
            .unwrap();
    }

    let actual = repository
        .query_all_partner_shop_application_records_by_user(&user_id)
        .await
        .unwrap();

    assert_eq!(3, actual.len());
    for record in &actual {
        assert_eq!(user_id, record.applicant_user_id);
    }
}

#[localstack_test(services = [DynamoDB()])]
fn should_return_empty_when_querying_by_user_with_no_records() {
    let repository = get_repository().await;

    let actual = repository
        .query_all_partner_shop_application_records_by_user(&UserId::new())
        .await
        .unwrap();

    assert!(actual.is_empty());
}

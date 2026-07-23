use common::shop_id::ShopId;
use fake::{Fake, Faker};
use test_api::*;
use user::dynamodb::{
    repository::{UserDynamoDbRepository, UserDynamoDbRepositoryImpl},
    user_record::UserRecord,
    user_record_update::UserRecordUpdate,
};

#[aura_integration_test(services = [DynamoDB()])]
async fn should_return_none_when_not_exists() {
    let repository = UserDynamoDbRepositoryImpl::new(get_dynamodb_client().await, "table_1");

    let actual = repository.get_user_record(&Faker.fake()).await.unwrap();

    assert!(actual.is_none());
}

#[aura_integration_test(services = [DynamoDB()])]
async fn should_return_some_when_exists() {
    let repository = UserDynamoDbRepositoryImpl::new(get_dynamodb_client().await, "table_1");
    let expected = Faker.fake::<UserRecord>();

    let _ = repository.put_user_record(expected.clone()).await.unwrap();

    let actual = repository
        .get_user_record(&expected.user_id)
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
#[aura_integration_test(services = [DynamoDB()])]
async fn should_update_user_record(#[case] user_record_update: UserRecordUpdate) {
    let repository = UserDynamoDbRepositoryImpl::new(get_dynamodb_client().await, "table_1");
    let initial = Faker.fake::<UserRecord>();

    let _ = repository.put_user_record(initial.clone()).await.unwrap();

    let updated = repository
        .update_user_record(&initial.user_id, user_record_update.clone())
        .await
        .unwrap()
        .unwrap();
    let actual = repository
        .get_user_record(&initial.user_id)
        .await
        .unwrap()
        .unwrap();

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
    if let Some(prohibited_content_consent) = user_record_update.prohibited_content_consent {
        assert_eq!(
            prohibited_content_consent,
            updated.prohibited_content_consent
        );
    }

    assert_eq!(updated, actual);
}

#[aura_integration_test(services = [DynamoDB()])]
async fn should_delete_user_record_when_exists() {
    let repository = UserDynamoDbRepositoryImpl::new(get_dynamodb_client().await, "table_1");
    let record = Faker.fake::<UserRecord>();

    let _ = repository.put_user_record(record.clone()).await.unwrap();

    let before = repository.get_user_record(&record.user_id).await.unwrap();
    assert!(before.is_some());

    let _ = repository
        .delete_user_record(&record.user_id)
        .await
        .unwrap();

    let after = repository.get_user_record(&record.user_id).await.unwrap();
    assert!(after.is_none());
}

#[aura_integration_test(services = [DynamoDB()])]
async fn should_not_error_when_deleting_non_existent_user_record() {
    let repository = UserDynamoDbRepositoryImpl::new(get_dynamodb_client().await, "table_1");

    let result = repository.delete_user_record(&Faker.fake()).await;

    assert!(result.is_ok());
}

#[aura_integration_test(services = [DynamoDB()])]
async fn should_find_user_record_by_stripe_customer_id_when_set() {
    use common::stripe_customer_id::StripeCustomerId;
    let repository = UserDynamoDbRepositoryImpl::new(get_dynamodb_client().await, "table_1");
    let stripe_customer_id = StripeCustomerId::from(format!("cus_{}", uuid::Uuid::new_v4()));
    let mut record = Faker.fake::<UserRecord>();
    record.stripe_customer_id = Some(stripe_customer_id.clone());
    record.gsi1_pk = Some(user::dynamodb::user_record::mk_gsi1_pk(&stripe_customer_id));
    record.gsi1_sk = Some(user::dynamodb::user_record::mk_gsi1_sk().to_owned());

    let _ = repository.put_user_record(record.clone()).await.unwrap();

    let actual = repository
        .find_user_record_by_stripe_customer_id(&stripe_customer_id)
        .await
        .unwrap()
        .unwrap();

    assert_eq!(record.user_id, actual.user_id);
    assert_eq!(Some(stripe_customer_id), actual.stripe_customer_id);
}

// ---------------------------------------------------------------------------
// add_partner_shop
// ---------------------------------------------------------------------------

#[aura_integration_test(services = [DynamoDB()])]
async fn should_add_shop_id_to_partner_shops_when_adding_to_empty_set() {
    let repository = UserDynamoDbRepositoryImpl::new(get_dynamodb_client().await, "table_1");
    let shop_id: ShopId = Faker.fake();
    let mut record = Faker.fake::<UserRecord>();
    record.partner_shops = Default::default();
    let user_id = record.user_id;

    repository.put_user_record(record).await.unwrap();
    repository
        .add_partner_shop(&user_id, &shop_id)
        .await
        .unwrap();

    let updated = repository.get_user_record(&user_id).await.unwrap().unwrap();
    assert_eq!(updated.partner_shops, std::iter::once(shop_id).collect());
}

#[aura_integration_test(services = [DynamoDB()])]
async fn should_preserve_existing_shop_when_adding_new_shop_to_partner_shops() {
    let repository = UserDynamoDbRepositoryImpl::new(get_dynamodb_client().await, "table_1");
    let shop_id_1: ShopId = Faker.fake();
    let shop_id_2: ShopId = Faker.fake();
    let mut record = Faker.fake::<UserRecord>();
    record.partner_shops = std::iter::once(shop_id_1).collect();
    let user_id = record.user_id;

    repository.put_user_record(record).await.unwrap();
    repository
        .add_partner_shop(&user_id, &shop_id_2)
        .await
        .unwrap();

    let updated = repository.get_user_record(&user_id).await.unwrap().unwrap();
    assert!(
        updated.partner_shops.contains(&shop_id_1),
        "original shop_id must still be present"
    );
    assert!(
        updated.partner_shops.contains(&shop_id_2),
        "new shop_id must be added"
    );
    assert_eq!(updated.partner_shops.len(), 2);
}

#[aura_integration_test(services = [DynamoDB()])]
async fn should_be_idempotent_when_adding_same_shop_to_partner_shops_twice() {
    let repository = UserDynamoDbRepositoryImpl::new(get_dynamodb_client().await, "table_1");
    let shop_id: ShopId = Faker.fake();
    let mut record = Faker.fake::<UserRecord>();
    record.partner_shops = Default::default();
    let user_id = record.user_id;

    repository.put_user_record(record).await.unwrap();
    repository
        .add_partner_shop(&user_id, &shop_id)
        .await
        .unwrap();
    repository
        .add_partner_shop(&user_id, &shop_id)
        .await
        .unwrap();

    let updated = repository.get_user_record(&user_id).await.unwrap().unwrap();
    assert_eq!(
        updated.partner_shops,
        std::iter::once(shop_id).collect(),
        "set must contain exactly one entry"
    );
}

#[aura_integration_test(services = [DynamoDB()])]
async fn should_return_none_when_finding_user_by_unknown_stripe_customer_id() {
    use common::stripe_customer_id::StripeCustomerId;
    let repository = UserDynamoDbRepositoryImpl::new(get_dynamodb_client().await, "table_1");

    let actual = repository
        .find_user_record_by_stripe_customer_id(&StripeCustomerId::from(format!(
            "cus_{}",
            uuid::Uuid::new_v4()
        )))
        .await
        .unwrap();

    assert!(actual.is_none());
}

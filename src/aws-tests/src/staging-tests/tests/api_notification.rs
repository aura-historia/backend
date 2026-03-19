use aws_tests_common::get_cfn_output;
use common::event_id::EventId;
use fake::{Fake, Faker};
use notification::data::get_notification_data::GetNotificationData;
use notification::data::patch_notification_data::PatchNotificationData;
use notification::dynamodb::notification_record::NotificationRecord;
use notification::dynamodb::repository::{
    NotificationDynamoDbRepository, NotificationDynamoDbRepositoryImpl,
};
use notification_api::notification_get::EventIdCursoredData;
use staging_tests::{create_random_test_user, get_dynamodb_client, staging_test};

async fn get_notification_repository() -> NotificationDynamoDbRepositoryImpl<'static> {
    NotificationDynamoDbRepositoryImpl::new(
        get_dynamodb_client().await,
        &get_cfn_output().dynamodb_table_1_name,
    )
}

#[staging_test]
async fn should_401_when_unauthorized_for_get() {
    let url = format!(
        "{}/api/v1/me/notifications",
        get_cfn_output().api_gateway_endpoint_url,
    );
    let response = reqwest::Client::new().get(url).send().await.unwrap();
    assert_eq!(401, response.status());
}

#[staging_test]
async fn should_401_when_unauthorized_for_patch_all() {
    let url = format!(
        "{}/api/v1/me/notifications",
        get_cfn_output().api_gateway_endpoint_url,
    );
    let response = reqwest::Client::new().patch(url).send().await.unwrap();
    assert_eq!(401, response.status());
}

#[staging_test]
async fn should_401_when_unauthorized_for_patch_one() {
    let url = format!(
        "{}/api/v1/me/notifications/{}",
        get_cfn_output().api_gateway_endpoint_url,
        EventId::new(),
    );
    let response = reqwest::Client::new().patch(url).send().await.unwrap();
    assert_eq!(401, response.status());
}

#[staging_test]
async fn should_401_when_unauthorized_for_delete_all() {
    let url = format!(
        "{}/api/v1/me/notifications",
        get_cfn_output().api_gateway_endpoint_url,
    );
    let response = reqwest::Client::new().delete(url).send().await.unwrap();
    assert_eq!(401, response.status());
}

#[staging_test]
async fn should_401_when_unauthorized_for_delete_one() {
    let url = format!(
        "{}/api/v1/me/notifications/{}",
        get_cfn_output().api_gateway_endpoint_url,
        EventId::new(),
    );
    let response = reqwest::Client::new().delete(url).send().await.unwrap();
    assert_eq!(401, response.status());
}

#[staging_test]
async fn should_get_and_patch_one_and_patch_all_and_delete_one_and_delete_all_notifications() {
    let user = create_random_test_user().await;
    let repository = get_notification_repository().await;

    // Seed some notifications directly via DynamoDB
    let mut record1 = Faker.fake::<NotificationRecord>();
    record1.pk = notification::dynamodb::notification_record::mk_pk(&user.sub.into());
    record1.user_id = user.sub.into();
    record1.seen = false;
    repository
        .put_notification_record(record1.clone())
        .await
        .unwrap();

    let mut record2 = Faker.fake::<NotificationRecord>();
    record2.pk = notification::dynamodb::notification_record::mk_pk(&user.sub.into());
    record2.user_id = user.sub.into();
    record2.seen = false;
    repository
        .put_notification_record(record2.clone())
        .await
        .unwrap();

    // GET notifications
    let get_url = format!(
        "{}/api/v1/me/notifications",
        get_cfn_output().api_gateway_endpoint_url,
    );
    let get_response = reqwest::Client::new()
        .get(&get_url)
        .bearer_auth(&user.access_token)
        .send()
        .await
        .unwrap();
    assert_eq!(200, get_response.status());
    let gotten = get_response
        .json::<EventIdCursoredData<GetNotificationData>>()
        .await
        .unwrap();
    assert_eq!(2, gotten.items.len());
    // FIXME: issue#650
    // assert_eq!(Some(2), gotten.total);
    assert!(gotten.items.iter().all(|n| !n.seen));

    // PATCH one notification (mark as seen)
    let patch_one_url = format!(
        "{}/api/v1/me/notifications/{}",
        get_cfn_output().api_gateway_endpoint_url,
        record1.origin_event_id,
    );
    let patch_one_response = reqwest::Client::new()
        .patch(patch_one_url)
        .bearer_auth(&user.access_token)
        .json(&PatchNotificationData { seen: Some(true) })
        .send()
        .await
        .unwrap();
    assert_eq!(200, patch_one_response.status());
    let patched_one = patch_one_response
        .json::<GetNotificationData>()
        .await
        .unwrap();
    assert_eq!(record1.origin_event_id, patched_one.origin_event_id);
    assert!(patched_one.seen);

    // PATCH all notifications (mark all as seen)
    let patch_all_url = format!(
        "{}/api/v1/me/notifications",
        get_cfn_output().api_gateway_endpoint_url,
    );
    let patch_all_response = reqwest::Client::new()
        .patch(&patch_all_url)
        .bearer_auth(&user.access_token)
        .send()
        .await
        .unwrap();
    assert_eq!(200, patch_all_response.status());
    let patched_all = patch_all_response
        .json::<EventIdCursoredData<GetNotificationData>>()
        .await
        .unwrap();
    assert!(patched_all.items.iter().all(|n| n.seen));

    // DELETE one notification
    let delete_one_url = format!(
        "{}/api/v1/me/notifications/{}",
        get_cfn_output().api_gateway_endpoint_url,
        record1.origin_event_id,
    );
    let delete_one_response = reqwest::Client::new()
        .delete(delete_one_url)
        .bearer_auth(&user.access_token)
        .send()
        .await
        .unwrap();
    assert_eq!(204, delete_one_response.status());

    // GET after delete-one: only 1 remains
    let get_response = reqwest::Client::new()
        .get(&get_url)
        .bearer_auth(&user.access_token)
        .send()
        .await
        .unwrap();
    assert_eq!(200, get_response.status());
    let after_delete_one = get_response
        .json::<EventIdCursoredData<GetNotificationData>>()
        .await
        .unwrap();
    assert_eq!(1, after_delete_one.items.len());

    // PATCH all (mark all as seen - no body needed since service updates all)
    let patch_all_specific_response = reqwest::Client::new()
        .patch(&patch_all_url)
        .bearer_auth(&user.access_token)
        .send()
        .await
        .unwrap();
    assert_eq!(200, patch_all_specific_response.status());

    // DELETE all notifications
    let delete_all_url = format!(
        "{}/api/v1/me/notifications",
        get_cfn_output().api_gateway_endpoint_url,
    );
    let delete_all_response = reqwest::Client::new()
        .delete(delete_all_url)
        .bearer_auth(&user.access_token)
        .send()
        .await
        .unwrap();
    assert_eq!(204, delete_all_response.status());

    // GET after delete-all: none remain
    let get_response = reqwest::Client::new()
        .get(&get_url)
        .bearer_auth(&user.access_token)
        .send()
        .await
        .unwrap();
    assert_eq!(200, get_response.status());
    let after_delete_all = get_response
        .json::<EventIdCursoredData<GetNotificationData>>()
        .await
        .unwrap();
    assert!(after_delete_all.items.is_empty());
    // FIXME: issue#650
    // assert_eq!(0, after_delete_all.total.unwrap_or(0));
}

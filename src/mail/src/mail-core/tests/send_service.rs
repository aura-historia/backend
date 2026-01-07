use aws_sdk_s3::{
    operation::get_object::GetObjectOutput,
    primitives::{ByteStream, SdkBody},
};
use aws_sdk_sesv2::operation::send_email::SendEmailOutput;
use fake::{Fake, Faker};
use mail_core::{
    payload::MailPayload,
    record::MailRecord,
    repository::{MailDynamoDbRepository, MailDynamoDbRepositoryImpl},
    s3_adapter::MockS3Adapter,
    send_service::{SendMailService, SendMailServiceImpl},
    ses_adapter::MockSesAdapter,
};
use test_api::*;

#[localstack_test(services = [DynamoDB()])]
async fn should_send_mail_when_not_exists() {
    let mail_repository = MailDynamoDbRepositoryImpl::new(get_dynamodb_client().await, "table_1");
    let mut s3_adapter = MockS3Adapter::default();
    s3_adapter.expect_get_object().return_once(|_, _| {
        Box::pin(async {
            Ok(GetObjectOutput::builder()
                .body(ByteStream::new(SdkBody::empty()))
                .build())
        })
    });
    let mut ses_adapter = MockSesAdapter::default();
    ses_adapter
        .expect_send_email()
        .return_once(|_, _, _| Box::pin(async { Ok(SendEmailOutput::builder().build()) }));

    let service = SendMailServiceImpl::new(
        &mail_repository,
        &ses_adapter,
        &s3_adapter,
        "foo",
        "moo",
        "boo",
    );

    let payload = Faker.fake::<MailPayload>();
    service.send_mail(payload.clone()).await.unwrap();

    assert!(
        mail_repository
            .get_mail_record(&payload.user_id, &payload.mail_id)
            .await
            .unwrap()
            .is_some()
    );
}

#[localstack_test(services = [DynamoDB()])]
async fn should_not_send_mail_when_exists() {
    let mail_repository = MailDynamoDbRepositoryImpl::new(get_dynamodb_client().await, "table_1");
    let mut s3_adapter = MockS3Adapter::default();
    s3_adapter.expect_get_object().return_once(|_, _| {
        Box::pin(async {
            Ok(GetObjectOutput::builder()
                .body(ByteStream::new(SdkBody::empty()))
                .build())
        })
    });
    let mut ses_adapter = MockSesAdapter::default();
    ses_adapter.expect_send_email().never();

    let service = SendMailServiceImpl::new(
        &mail_repository,
        &ses_adapter,
        &s3_adapter,
        "foo",
        "moo",
        "boo",
    );

    let payload = Faker.fake::<MailPayload>();
    let expected = MailRecord::from(payload.clone());
    mail_repository
        .put_mail_record(expected.clone())
        .await
        .unwrap();
    service.send_mail(payload.clone()).await.unwrap();

    assert_eq!(
        expected,
        mail_repository
            .get_mail_record(&payload.user_id, &payload.mail_id)
            .await
            .unwrap()
            .unwrap()
    );
}

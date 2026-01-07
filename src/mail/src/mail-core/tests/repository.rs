use mail_core::{
    record::MailRecord,
    repository::{MailDynamoDbRepository, MailDynamoDbRepositoryImpl},
};
use test_api::*;

#[localstack_test(services = [DynamoDB()])]
async fn should_put_then_get_mail_record() {
    let repository = MailDynamoDbRepositoryImpl::new(get_dynamodb_client().await, "table_1");

    let expecteds = fake::vec![MailRecord; 42];
    for mail_record in expecteds.iter().cloned() {
        repository.put_mail_record(mail_record).await.unwrap();
    }

    for expected in expecteds {
        let actual = repository
            .get_mail_record(&expected.user_id, &expected.mail_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(expected, actual);
    }
}

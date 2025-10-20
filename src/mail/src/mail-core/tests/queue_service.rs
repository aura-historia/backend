use mail_core::{
    payload::MailPayload,
    queue_service::{QueueMailService, QueueMailServiceImpl},
};
use test_api::*;

const MAIL_QUEUE: Sqs = Sqs { name: "mail-queue" };

#[localstack_test(services = [MAIL_QUEUE])]
async fn should_push_mail_payloads_to_queue() {
    let sqs_client = get_sqs_client().await;
    let q_url = MAIL_QUEUE.queue_url();
    let service = QueueMailServiceImpl::new(sqs_client, &q_url);

    let failed = service.queue_mails(fake::vec![MailPayload; 42]).await;
    assert!(failed.is_empty());

    let mut received_count = 0;
    loop {
        let received = sqs_client
            .receive_message()
            .queue_url(&q_url)
            .max_number_of_messages(10)
            .visibility_timeout(600)
            .send()
            .await
            .unwrap();

        match received.messages.unwrap_or_default().as_slice() {
            &[] => break,
            msgs => received_count += msgs.len(),
        }
    }

    assert_eq!(42, received_count);
}

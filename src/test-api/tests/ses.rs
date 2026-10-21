use aws_sdk_sesv2::types::{Body, Content, Destination, EmailContent, Message};
use test_api::{
    IntegrationTestService, Ses, aura_integration_test, clear_sent_emails, get_sent_emails,
    get_sent_emails_from, get_ses_client, verify_sender_identity,
};

async fn send(source: &str) -> String {
    get_ses_client()
        .await
        .send_email()
        .from_email_address(source)
        .destination(
            Destination::builder()
                .to_addresses("recipient@example.test")
                .build(),
        )
        .content(
            EmailContent::builder()
                .simple(
                    Message::builder()
                        .subject(Content::builder().data("Fixture subject").build().unwrap())
                        .body(
                            Body::builder()
                                .text(Content::builder().data("Fixture text").build().unwrap())
                                .html(
                                    Content::builder()
                                        .data("<p>Fixture HTML</p>")
                                        .build()
                                        .unwrap(),
                                )
                                .build(),
                        )
                        .build(),
                )
                .build(),
        )
        .send()
        .await
        .unwrap()
        .message_id()
        .expect("SES must return a message ID")
        .to_owned()
}

#[aura_integration_test(services = [Ses()], isolated = true)]
async fn should_capture_message_content_and_clear_it() {
    let source = test_api::TestContext::current()
        .unwrap()
        .sender_email()
        .unwrap();
    verify_sender_identity(&source).await;
    let id = send(&source).await;
    let emails = get_sent_emails().await;
    assert_eq!(1, emails.len());
    let email = &emails[0];
    assert_eq!(id, email.id);
    assert_eq!("eu-central-1", email.region);
    assert_eq!(source, email.source);
    assert_eq!(
        ["recipient@example.test"],
        email.destination.to_addresses.as_slice()
    );
    assert_eq!("Fixture subject", email.subject);
    assert_eq!(Some("Fixture text"), email.body.text_part.as_deref());
    assert_eq!(Some("<p>Fixture HTML</p>"), email.body.html_part.as_deref());

    clear_sent_emails().await;
    assert!(get_sent_emails().await.is_empty());
}

#[aura_integration_test(services = [Ses()], isolated = true)]
async fn should_filter_captured_messages_by_source() {
    let context = test_api::TestContext::current().unwrap();
    let source = context.sender_email().unwrap();
    let alternate = context.register_sender("secondary").await.unwrap();
    verify_sender_identity(&alternate).await;
    send(&source).await;
    let id = send(&alternate).await;

    let emails = get_sent_emails_from(&alternate).await;
    assert_eq!(1, emails.len());
    assert_eq!(id, emails[0].id);
    assert_eq!(alternate, emails[0].source);
}

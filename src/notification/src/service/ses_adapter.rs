use aws_sdk_sesv2::{
    Client,
    error::SdkError,
    operation::send_email::{SendEmailError, SendEmailOutput},
    types::{Destination, EmailContent, MessageTag},
};
use serde_email::Email;

#[async_trait::async_trait]
#[mockall::automock]
pub trait SesAdapter {
    async fn send_email(
        &self,
        from: Email,
        to: Email,
        reply_to: Email,
        content: EmailContent,
        tags: Vec<MessageTag>,
    ) -> Result<SendEmailOutput, SdkError<SendEmailError>>;
}

#[derive(Clone)]
pub struct SesAdapterImpl<'a> {
    client: &'a Client,
}

impl<'a> SesAdapterImpl<'a> {
    pub fn new(client: &'a Client) -> Self {
        Self { client }
    }
}

#[async_trait::async_trait]
impl<'a> SesAdapter for SesAdapterImpl<'a> {
    async fn send_email(
        &self,
        from: Email,
        to: Email,
        reply_to: Email,
        content: EmailContent,
        tags: Vec<MessageTag>,
    ) -> Result<SendEmailOutput, SdkError<SendEmailError>> {
        self.client
            .send_email()
            .from_email_address(from)
            .reply_to_addresses(reply_to)
            .destination(Destination::builder().to_addresses(to).build())
            .content(content)
            .set_email_tags(Some(tags))
            .send()
            .await
    }
}

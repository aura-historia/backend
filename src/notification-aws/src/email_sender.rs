use crate::{
    mapping::{template_data, template_type},
    template_reader::TemplateReader,
};
use aws_sdk_s3::Client as S3Client;
use aws_sdk_sesv2::{
    Client as SesClient,
    types::{Body, Content, Destination, EmailContent, Message, MessageTag},
};
use common::error::boxed::box_error;
use notification_service::ports::{
    notification_delivery_repository::NotificationDeliverySource,
    notification_delivery_sender::{
        NotificationDeliverySendError, NotificationDeliverySender, SentNotificationDelivery,
    },
};

pub struct SesNotificationDeliverySender {
    ses: SesClient,
    from_email_address: String,
    reply_to_email_address: String,
    templates: TemplateReader,
}

impl SesNotificationDeliverySender {
    pub fn new(
        s3: S3Client,
        ses: SesClient,
        template_bucket: impl Into<String>,
        from_email_address: impl Into<String>,
        reply_to_email_address: impl Into<String>,
        stage: impl Into<String>,
        commit_sha: impl Into<String>,
    ) -> Self {
        Self {
            ses,
            from_email_address: from_email_address.into(),
            reply_to_email_address: reply_to_email_address.into(),
            templates: TemplateReader::new(s3, template_bucket, stage, commit_sha),
        }
    }

    async fn render(
        &self,
        source: &NotificationDeliverySource,
    ) -> Result<(String, String, &'static str), NotificationDeliverySendError> {
        let template_type = template_type(&source.content);
        let body = self
            .templates
            .render(template_type, source.language, &template_data(source))
            .await?;

        Ok((
            crate::mapping::subject(template_type).to_owned(),
            body,
            crate::mapping::ses_template_tag_value(template_type),
        ))
    }
}

#[async_trait::async_trait]
impl NotificationDeliverySender for SesNotificationDeliverySender {
    async fn send(
        &self,
        source: &NotificationDeliverySource,
    ) -> Result<SentNotificationDelivery, NotificationDeliverySendError> {
        let (subject, body, template_tag_value) = self.render(source).await?;
        let message = Message::builder()
            .subject(Content::builder().data(subject).build().map_err(|source| {
                NotificationDeliverySendError::Permanent {
                    code: "EMAIL_CONTENT_INVALID",
                    source: box_error(source),
                }
            })?)
            .body(
                Body::builder()
                    .html(Content::builder().data(body).build().map_err(|source| {
                        NotificationDeliverySendError::Permanent {
                            code: "EMAIL_CONTENT_INVALID",
                            source: box_error(source),
                        }
                    })?)
                    .build(),
            )
            .build();
        let email_tag = MessageTag::builder()
            .name("template_type")
            .value(template_tag_value)
            .build()
            .map_err(|source| NotificationDeliverySendError::Permanent {
                code: "EMAIL_TAG_INVALID",
                source: box_error(source),
            })?;
        let response = self
            .ses
            .send_email()
            .from_email_address(&self.from_email_address)
            .reply_to_addresses(&self.reply_to_email_address)
            .destination(
                Destination::builder()
                    .to_addresses(&source.recipient_email)
                    .build(),
            )
            .content(EmailContent::builder().simple(message).build())
            .email_tags(email_tag)
            .send()
            .await
            .map_err(|source| NotificationDeliverySendError::Retryable {
                code: "SES_SEND_FAILED",
                source: box_error(source),
            })?;
        let provider_message_id =
            response
                .message_id()
                .ok_or_else(|| NotificationDeliverySendError::Retryable {
                    code: "SES_MESSAGE_ID_MISSING",
                    source: box_error(std::io::Error::other(
                        "SES response did not include a message ID",
                    )),
                })?;

        Ok(SentNotificationDelivery {
            provider_message_id: provider_message_id.to_owned(),
        })
    }
}

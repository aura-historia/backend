use crate::{
    template_mapping::{ses_template_tag_value, subject, template_data, template_type},
    template_reader::TemplateReader,
};
use aws_sdk_s3::Client as S3Client;
use aws_sdk_sesv2::{
    Client as SesClient,
    types::{Body, Content, Destination, EmailContent, Message, MessageTag},
};
use common::error::boxed::box_error;
use notification_core::notification_delivery::NotificationDeliveryChannel;
use notification_service::ports::{
    email_delivery_target_reader::{EmailDeliveryTargetReadError, EmailDeliveryTargetReader},
    notification_channel_sender::{
        NotificationChannelSendError, NotificationChannelSender, SentNotificationDelivery,
    },
    notification_delivery_repository::NotificationDeliverySource,
};
use std::sync::Arc;

pub struct EmailDeliveryConfig {
    template_bucket: String,
    from_email_address: String,
    reply_to_email_address: String,
    stage: String,
    commit_sha: String,
}

impl EmailDeliveryConfig {
    pub fn new(
        template_bucket: impl Into<String>,
        from_email_address: impl Into<String>,
        reply_to_email_address: impl Into<String>,
        stage: impl Into<String>,
        commit_sha: impl Into<String>,
    ) -> Self {
        Self {
            template_bucket: template_bucket.into(),
            from_email_address: from_email_address.into(),
            reply_to_email_address: reply_to_email_address.into(),
            stage: stage.into(),
            commit_sha: commit_sha.into(),
        }
    }
}

pub struct SesNotificationChannelSender {
    ses: SesClient,
    from_email_address: String,
    reply_to_email_address: String,
    templates: TemplateReader,
    targets: Arc<dyn EmailDeliveryTargetReader>,
}

impl SesNotificationChannelSender {
    pub fn new(
        s3: S3Client,
        ses: SesClient,
        config: EmailDeliveryConfig,
        targets: Arc<dyn EmailDeliveryTargetReader>,
    ) -> Self {
        Self {
            ses,
            from_email_address: config.from_email_address,
            reply_to_email_address: config.reply_to_email_address,
            templates: TemplateReader::new(
                s3,
                config.template_bucket,
                config.stage,
                config.commit_sha,
            ),
            targets,
        }
    }

    async fn render(
        &self,
        source: &NotificationDeliverySource,
        first_name: Option<&str>,
    ) -> Result<(String, String, &'static str), NotificationChannelSendError> {
        let template_type = template_type(&source.content);
        let body = self
            .templates
            .render(
                template_type,
                source.language,
                &template_data(source, first_name),
            )
            .await?;
        Ok((
            subject(template_type, source.language).to_owned(),
            body,
            ses_template_tag_value(template_type),
        ))
    }
}

#[async_trait::async_trait]
impl NotificationChannelSender for SesNotificationChannelSender {
    fn channel(&self) -> NotificationDeliveryChannel {
        NotificationDeliveryChannel::Email
    }

    async fn send(
        &self,
        source: &NotificationDeliverySource,
    ) -> Result<SentNotificationDelivery, NotificationChannelSendError> {
        let target = self
            .targets
            .find_email_target(source.user_id, &source.target_key)
            .await
            .map_err(target_error)?
            .ok_or_else(|| NotificationChannelSendError::Permanent {
                code: "EMAIL_TARGET_MISSING",
                source: box_error(std::io::Error::other("email delivery target is missing")),
            })?;
        let (subject, body, template_tag_value) =
            self.render(source, target.first_name.as_deref()).await?;
        let message = Message::builder()
            .subject(Content::builder().data(subject).build().map_err(|source| {
                NotificationChannelSendError::Permanent {
                    code: "EMAIL_CONTENT_INVALID",
                    source: box_error(source),
                }
            })?)
            .body(
                Body::builder()
                    .html(Content::builder().data(body).build().map_err(|source| {
                        NotificationChannelSendError::Permanent {
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
            .map_err(|source| NotificationChannelSendError::Permanent {
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
                    .to_addresses(target.address.to_string())
                    .build(),
            )
            .content(EmailContent::builder().simple(message).build())
            .email_tags(email_tag)
            .send()
            .await
            .map_err(|source| NotificationChannelSendError::Retryable {
                code: "SES_SEND_FAILED",
                source: box_error(source),
            })?;
        let provider_message_id =
            response
                .message_id()
                .ok_or_else(|| NotificationChannelSendError::Retryable {
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

fn target_error(error: EmailDeliveryTargetReadError) -> NotificationChannelSendError {
    match error {
        EmailDeliveryTargetReadError::ReadFailed { source } => {
            NotificationChannelSendError::Retryable {
                code: "EMAIL_TARGET_READ_FAILED",
                source,
            }
        }
        EmailDeliveryTargetReadError::InvalidPersistedState { source } => {
            NotificationChannelSendError::Permanent {
                code: "EMAIL_TARGET_INVALID",
                source,
            }
        }
    }
}

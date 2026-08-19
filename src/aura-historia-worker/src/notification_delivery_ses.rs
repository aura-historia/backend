use aws_sdk_s3::Client as S3Client;
use aws_sdk_sesv2::{
    Client as SesClient,
    types::{Body, Content, Destination, EmailContent, Message, MessageTag},
};
use common::{error::boxed::box_error, language::domain::Language};
use handlebars::Handlebars;
use notification_core::{
    mail_template::{MailTemplate, MailTemplateType},
    notification::{NotificationContent, NotificationWatchlistChange},
};
use notification_service::ports::{
    notification_delivery_repository::NotificationDeliverySource,
    notification_delivery_sender::{
        NotificationDeliverySendError, NotificationDeliverySender, SentNotificationDelivery,
    },
};
use serde_json::{Value, json};

const SENDER_MAIL: &str = "no-reply@notify.aura-historia.com";
const REPLY_TO_MAIL: &str = "contact@aura-historia.com";

pub struct SesNotificationDeliverySender {
    s3: S3Client,
    ses: SesClient,
    template_bucket: String,
    stage_name: String,
    commit_sha: String,
    templates: Handlebars<'static>,
}

impl SesNotificationDeliverySender {
    pub fn new(
        s3: S3Client,
        ses: SesClient,
        template_bucket: String,
        stage_name: String,
        commit_sha: String,
    ) -> Self {
        Self {
            s3,
            ses,
            template_bucket,
            stage_name,
            commit_sha,
            templates: Handlebars::new(),
        }
    }

    async fn render(
        &self,
        source: &NotificationDeliverySource,
    ) -> Result<(String, String, MailTemplateType), NotificationDeliverySendError> {
        let template_type = template_type(&source.content);
        let template = MailTemplate {
            template_type,
            language: Language::En,
        };
        let key = format!(
            "{}/{}/{}.html",
            self.stage_name,
            self.commit_sha,
            template.as_s3_blob_str(),
        );
        let response = self
            .s3
            .get_object()
            .bucket(&self.template_bucket)
            .key(key)
            .send()
            .await
            .map_err(|error| NotificationDeliverySendError::Retryable {
                code: "S3_TEMPLATE_FETCH_FAILED",
                source: box_error(error),
            })?;
        let bytes = response
            .body
            .collect()
            .await
            .map_err(|error| NotificationDeliverySendError::Retryable {
                code: "S3_TEMPLATE_READ_FAILED",
                source: box_error(error),
            })?
            .into_bytes();
        let html = String::from_utf8(bytes.to_vec()).map_err(|error| {
            NotificationDeliverySendError::Permanent {
                code: "S3_TEMPLATE_INVALID_UTF8",
                source: box_error(error),
            }
        })?;
        let body = self
            .templates
            .render_template(&html, &template_data(&source.content))
            .map_err(|error| NotificationDeliverySendError::Permanent {
                code: "S3_TEMPLATE_RENDER_FAILED",
                source: box_error(error),
            })?;
        Ok((subject(template_type).to_owned(), body, template_type))
    }
}

#[async_trait::async_trait]
impl NotificationDeliverySender for SesNotificationDeliverySender {
    async fn send(
        &self,
        source: &NotificationDeliverySource,
    ) -> Result<SentNotificationDelivery, NotificationDeliverySendError> {
        let (subject, body, template_type) = self.render(source).await?;
        let message = Message::builder()
            .subject(Content::builder().data(subject).build().map_err(|error| {
                NotificationDeliverySendError::Permanent {
                    code: "EMAIL_CONTENT_INVALID",
                    source: box_error(error),
                }
            })?)
            .body(
                Body::builder()
                    .html(Content::builder().data(body).build().map_err(|error| {
                        NotificationDeliverySendError::Permanent {
                            code: "EMAIL_CONTENT_INVALID",
                            source: box_error(error),
                        }
                    })?)
                    .build(),
            )
            .build();
        let response = self
            .ses
            .send_email()
            .from_email_address(SENDER_MAIL)
            .reply_to_addresses(REPLY_TO_MAIL)
            .destination(
                Destination::builder()
                    .to_addresses(&source.recipient_email)
                    .build(),
            )
            .content(EmailContent::builder().simple(message).build())
            .email_tags(
                MessageTag::builder()
                    .name("template_type")
                    .value(template_type.as_message_tag_value())
                    .build()
                    .map_err(|error| NotificationDeliverySendError::Permanent {
                        code: "EMAIL_TAG_INVALID",
                        source: box_error(error),
                    })?,
            )
            .send()
            .await
            .map_err(|error| NotificationDeliverySendError::Retryable {
                code: "SES_SEND_FAILED",
                source: box_error(error),
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

fn template_type(content: &NotificationContent) -> MailTemplateType {
    match content {
        NotificationContent::Watchlist {
            change: NotificationWatchlistChange::PriceChange { .. },
            ..
        } => MailTemplateType::WatchlistUpdatePrice,
        NotificationContent::Watchlist { .. } => MailTemplateType::WatchlistUpdateState,
        NotificationContent::SearchFilter { .. } => MailTemplateType::SearchFilterMatch,
        NotificationContent::PartnerApplication {
            decision: notification_core::notification::PartnerApplicationDecision::Approved,
            ..
        } => MailTemplateType::PartnerApplicationApproval,
        NotificationContent::PartnerApplication { .. } => {
            MailTemplateType::PartnerApplicationRejection
        }
    }
}

fn subject(template_type: MailTemplateType) -> &'static str {
    match template_type {
        MailTemplateType::WatchlistUpdatePrice => "Your watchlist price changed",
        MailTemplateType::WatchlistUpdateState => "Your watchlist item changed",
        MailTemplateType::SearchFilterMatch => "New search filter match",
        MailTemplateType::PartnerApplicationApproval => "Partner application approved",
        MailTemplateType::PartnerApplicationRejection => "Partner application update",
    }
}

fn template_data(content: &NotificationContent) -> Value {
    match content {
        NotificationContent::Watchlist { snapshot, .. }
        | NotificationContent::SearchFilter { snapshot, .. } => json!({
            "shop_name": snapshot.shop_name.to_string(),
            "shop_slug_id": snapshot.shop_slug_id.to_string(),
            "product_slug_id": snapshot.product_slug_id.to_string(),
            "image_url": snapshot.image.as_ref().map(|image| image.url.to_string()),
            "view_url": snapshot.view_url.to_string(),
        }),
        NotificationContent::PartnerApplication { snapshot, .. } => json!({
            "shop_name": snapshot.shop_name.to_string(),
            "image_url": snapshot.image.as_ref().map(ToString::to_string),
        }),
    }
}

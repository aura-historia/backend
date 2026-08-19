use crate::mapping::s3_template_key;
use aws_sdk_s3::Client as S3Client;
use common::{error::boxed::box_error, language::domain::Language};
use handlebars::Handlebars;
use notification_core::mail_template::MailTemplateType;
use notification_service::ports::notification_delivery_sender::NotificationDeliverySendError;
use serde_json::Value;

pub(crate) struct TemplateReader {
    s3: S3Client,
    template_bucket: String,
    stage: String,
    commit_sha: String,
    handlebars: Handlebars<'static>,
}

impl TemplateReader {
    pub(crate) fn new(
        s3: S3Client,
        template_bucket: impl Into<String>,
        stage: impl Into<String>,
        commit_sha: impl Into<String>,
    ) -> Self {
        Self {
            s3,
            template_bucket: template_bucket.into(),
            stage: stage.into(),
            commit_sha: commit_sha.into(),
            handlebars: Handlebars::new(),
        }
    }

    pub(crate) async fn render(
        &self,
        template_type: MailTemplateType,
        language: Language,
        data: &Value,
    ) -> Result<String, NotificationDeliverySendError> {
        let key = s3_template_key(&self.stage, &self.commit_sha, template_type, language);
        let response = self
            .s3
            .get_object()
            .bucket(&self.template_bucket)
            .key(key)
            .send()
            .await
            .map_err(|source| NotificationDeliverySendError::Retryable {
                code: "S3_TEMPLATE_FETCH_FAILED",
                source: box_error(source),
            })?;
        let bytes = response
            .body
            .collect()
            .await
            .map_err(|source| NotificationDeliverySendError::Retryable {
                code: "S3_TEMPLATE_READ_FAILED",
                source: box_error(source),
            })?
            .into_bytes();
        let template = String::from_utf8(bytes.to_vec()).map_err(|source| {
            NotificationDeliverySendError::Permanent {
                code: "S3_TEMPLATE_INVALID_UTF8",
                source: box_error(source),
            }
        })?;

        render_template(&self.handlebars, &template, data)
    }
}

fn render_template(
    handlebars: &Handlebars<'static>,
    template: &str,
    data: &Value,
) -> Result<String, NotificationDeliverySendError> {
    handlebars
        .render_template(template, data)
        .map_err(|source| NotificationDeliverySendError::Permanent {
            code: "S3_TEMPLATE_RENDER_FAILED",
            source: box_error(source),
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn should_render_template_with_notification_data() -> Result<(), NotificationDeliverySendError>
    {
        let rendered = render_template(
            &Handlebars::new(),
            "Hello {{shop_name}}. New price: {{new_price}}.",
            &json!({ "shop_name": "Aster Antiques", "new_price": "12,00 €" }),
        )?;

        assert_eq!(rendered, "Hello Aster Antiques. New price: 12,00 €.");
        Ok(())
    }

    #[test]
    fn should_return_safe_code_when_template_cannot_render() {
        let result = render_template(&Handlebars::new(), "{{#if", &json!({}));

        assert!(matches!(
            result,
            Err(NotificationDeliverySendError::Permanent {
                code: "S3_TEMPLATE_RENDER_FAILED",
                ..
            })
        ));
    }
}

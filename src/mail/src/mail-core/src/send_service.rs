use crate::{
    payload::MailPayload, record::MailRecord, repository::MailDynamoDbRepository,
    template::MailTemplate,
};
use aws_sdk_dynamodb::operation::get_item::GetItemError;
use aws_sdk_s3::operation::get_object::GetObjectError;
use aws_sdk_sesv2::{
    error::SdkError,
    operation::send_email::SendEmailError,
    types::{Body, Content, Destination, EmailContent, Message},
};
use handlebars::Handlebars;
use once_cell::sync::OnceCell;
use std::{collections::HashMap, sync::Arc};
use tokio::sync::RwLock;
use tracing::{info, warn};

#[derive(thiserror::Error, Debug)]
pub enum MailServiceError {
    #[error("Encountered DynamoDB SdkError for GetItem: {0:?}")]
    SdkDynamoDbGetItemError(#[from] SdkError<GetItemError>),

    #[error("Encountered S3 SdkError for GetObject: {0:?}")]
    SdkS3GetObjectError(#[from] SdkError<GetObjectError>),

    #[error("Encountered SES SdkError for SendMail: {0:?}")]
    SdkSESSendMailError(#[from] SdkError<SendEmailError>),

    #[error("Encountered Handlebars-Error for Render: {0}")]
    HandlebarsRenderError(#[from] handlebars::RenderError),
}

#[async_trait::async_trait]
#[mockall::automock]
pub trait SendMailService {
    async fn send_mail(&self, payload: MailPayload) -> Result<(), MailServiceError>;
}

#[derive(Clone)]
pub struct SendMailServiceImpl<'a> {
    mail_repository: &'a (dyn MailDynamoDbRepository + Send + Sync),
    ses_client: &'a aws_sdk_sesv2::Client,
    s3_client: &'a aws_sdk_s3::Client,
    s3_bucket: &'a str,
    stage_name: &'a str,
    commit_sha: &'a str,
    handlebars: Handlebars<'a>,
}

static TEMPLATE_CACHE: OnceCell<Arc<RwLock<HashMap<MailTemplate, String>>>> = OnceCell::new();

impl<'a> SendMailServiceImpl<'a> {
    pub fn new(
        mail_repository: &'a (dyn MailDynamoDbRepository + Send + Sync),
        ses_client: &'a aws_sdk_sesv2::Client,
        s3_client: &'a aws_sdk_s3::Client,
        s3_bucket: &'a str,
        stage_name: &'a str,
        commit_sha: &'a str,
    ) -> Self {
        Self {
            mail_repository,
            ses_client,
            s3_client,
            s3_bucket,
            stage_name,
            commit_sha,
            handlebars: Handlebars::new(),
        }
    }

    async fn resolve_template(
        &self,
        template: MailTemplate,
    ) -> Result<String, SdkError<GetObjectError>> {
        let template_cache_rw =
            TEMPLATE_CACHE.get_or_init(|| Arc::new(RwLock::new(HashMap::new())));

        {
            let template_cache_r = template_cache_rw.read().await;
            if let Some(resolved) = template_cache_r.get(&template) {
                return Ok(resolved.clone());
            }
        }

        let resp = self
            .s3_client
            .get_object()
            .bucket(self.s3_bucket)
            .key(format!(
                "{}/{}/{}.html",
                self.stage_name,
                self.commit_sha,
                template.as_s3_blob_str()
            ))
            .send()
            .await?;
        let bytes = resp
            .body
            .collect()
            .await
            .map_err(SdkError::construction_failure)? // misusing 'ConstructionFailure(..)' yields compact code here
            .into_bytes();
        let template_html = String::from_utf8_lossy(&bytes).to_string();

        {
            let mut template_cache_w = template_cache_rw.write().await;
            template_cache_w.insert(template, template_html.clone());
        }

        Ok(template_html)
    }
}

#[async_trait::async_trait]
impl<'a> SendMailService for SendMailServiceImpl<'a> {
    async fn send_mail(&self, payload: MailPayload) -> Result<(), MailServiceError> {
        let mail_record_opt = self
            .mail_repository
            .get_mail_record(&payload.user_id, &payload.mail_id)
            .await?;
        if let Some(mail_record_opt) = mail_record_opt {
            info!(
                userId = %payload.user_id,
                mailId = %payload.mail_id,
                previouslySentTimestamp = %mail_record_opt.created,
                "Mail was has already been sent. Skipping."
            );
            return Ok(());
        }

        let template_html = self.resolve_template(payload.template).await?;
        let rendered_html = self
            .handlebars
            .render_template(&template_html, &payload.data)?;
        let subject = Content::builder()
            .data(payload.subject.clone())
            .build()
            .expect("shouldn't fail because 'data' was set explicitly");
        let body = Body::builder()
            .html(
                Content::builder()
                    .data(rendered_html)
                    .build()
                    .expect("shouldn't fail because 'data' was set explicitly"),
            )
            .build();
        let message = Message::builder().subject(subject).body(body).build();
        let content = EmailContent::builder().simple(message).build();

        self.ses_client
            .send_email()
            .from_email_address(payload.sender.clone())
            .destination(
                Destination::builder()
                    .to_addresses(payload.recipient.clone())
                    .build(),
            )
            .content(content)
            .send()
            .await?;

        let user_id = payload.user_id;
        let mail_record = MailRecord::from(payload);
        const MAX_RETRIES: u32 = 5;
        const BASE_DELAY_MS: u64 = 100;
        let mut retry_count = 0;
        loop {
            let put_res = self
                .mail_repository
                .put_mail_record(mail_record.clone())
                .await;
            match put_res {
                Ok(_) => {
                    break;
                }
                Err(err) => {
                    warn!(error = ?err, userId = %user_id, "Failed persisting sent email to DynamoDB.");
                }
            }
            if retry_count >= MAX_RETRIES {
                warn!(
                    userId = %user_id,
                    "Failed persisting sent email to DynamoDB after '{MAX_RETRIES}' retries.
                     This will lead to another email being sent to the user."
                );
                // Shouldn't happen all too often, rare ALO-Delivery acceptable here, mostly EO
                break;
            }

            retry_count += 1;
            let delay_ms = BASE_DELAY_MS * 2_u64.pow(retry_count - 1);
            tokio::time::sleep(tokio::time::Duration::from_millis(delay_ms)).await;
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use crate::{
        repository::MockMailDynamoDbRepository,
        send_service::{SendMailServiceImpl, TEMPLATE_CACHE},
        template::{MailTemplate, MailTemplateType},
    };
    use aws_config::{BehaviorVersion, SdkConfig};
    use aws_sdk_dynamodb::operation::put_item::PutItemOutput;
    use common::language::data::LanguageData;
    use std::{collections::HashMap, sync::Arc};
    use tokio::sync::RwLock;

    #[tokio::test]
    async fn should_reuse_template_when_in_cache() {
        // Dummy-Config - test would use dummy-clients and therefore err if we didn't use cache
        let sdk_config = SdkConfig::builder()
            .behavior_version(BehaviorVersion::latest())
            .build();
        let ses_client = aws_sdk_sesv2::Client::new(&sdk_config);
        let s3_client = aws_sdk_s3::Client::new(&sdk_config);
        let mut mail_repository = MockMailDynamoDbRepository::default();
        mail_repository
            .expect_get_mail_record()
            .returning(|_, _| Box::pin(async { Ok(None) }));
        mail_repository
            .expect_put_mail_record()
            .returning(|_| Box::pin(async { Ok(PutItemOutput::builder().build()) }));
        let service = SendMailServiceImpl::new(
            &mail_repository,
            &ses_client,
            &s3_client,
            "foo",
            "moo",
            "boo",
        );

        TEMPLATE_CACHE.get_or_init(|| {
            Arc::new(RwLock::new(HashMap::from_iter([(
                MailTemplate {
                    template_type: MailTemplateType::WatchlistUpdatePrice,
                    language: LanguageData::De,
                },
                "bar".to_owned(),
            )])))
        });

        let actual = service
            .resolve_template(MailTemplate {
                template_type: MailTemplateType::WatchlistUpdatePrice,
                language: LanguageData::De,
            })
            .await
            .unwrap();

        assert_eq!("bar", actual);
    }
}

use crate::{payload::MailPayload, template::MailTemplate};
use aws_sdk_s3::operation::get_object::GetObjectError;
use aws_sdk_sesv2::{
    error::SdkError,
    operation::send_email::SendEmailError,
    types::{Body, Content, Destination, EmailContent, Message},
};
use handlebars::Handlebars;
use std::collections::HashMap;

#[derive(thiserror::Error, Debug)]
pub enum MailServiceError {
    #[error("Encountered S3 SdkError for GetObject: {0}")]
    SdkS3GetObjectError(#[from] SdkError<GetObjectError>),

    #[error("Encountered SES SdkError for SendMail: {0}")]
    SdkSESSendMailError(#[from] SdkError<SendEmailError>),

    #[error("Encountered Handlebars-Error for Render: {0}")]
    HandlebarsRenderError(#[from] handlebars::RenderError),
}

#[async_trait::async_trait]
#[mockall::automock]
pub trait MailService {
    async fn send(
        &self,
        payload: MailPayload,
        template_cache: &mut HashMap<MailTemplate, String>,
    ) -> Result<(), MailServiceError>;
}

#[derive(Debug, Clone)]
pub struct MailServiceImpl<'a> {
    ses_client: &'a aws_sdk_sesv2::Client,
    s3_client: &'a aws_sdk_s3::Client,
    s3_bucket: &'a str,
    handlebars: Handlebars<'a>,
}

impl<'a> MailServiceImpl<'a> {
    pub fn new(
        ses_client: &'a aws_sdk_sesv2::Client,
        s3_client: &'a aws_sdk_s3::Client,
        s3_bucket: &'a str,
    ) -> Self {
        Self {
            ses_client,
            s3_client,
            s3_bucket,
            handlebars: Handlebars::new(),
        }
    }

    async fn resolve_template(
        &self,
        template: MailTemplate,
        template_cache: &'a mut HashMap<MailTemplate, String>,
    ) -> Result<String, SdkError<GetObjectError>> {
        if let Some(resolved) = template_cache.get(&template) {
            Ok(resolved.clone())
        } else {
            let resp = self
                .s3_client
                .get_object()
                .bucket(self.s3_bucket)
                .key(format!("{}.html", template.as_str()))
                .send()
                .await?;

            let bytes = resp
                .body
                .collect()
                .await
                .map_err(SdkError::construction_failure)?
                .into_bytes();
            let html = String::from_utf8_lossy(&bytes).to_string();

            Ok(html)
        }
    }
}

#[async_trait::async_trait]
impl<'a> MailService for MailServiceImpl<'a> {
    async fn send(
        &self,
        payload: MailPayload,
        template_cache: &mut HashMap<MailTemplate, String>,
    ) -> Result<(), MailServiceError> {
        let template_html = self
            .resolve_template(payload.template, template_cache)
            .await?;
        let rendered_html = self
            .handlebars
            .render_template(&template_html, &payload.data)?;
        let subject = Content::builder()
            .data(payload.subject)
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
            .from_email_address(payload.sender)
            .destination(
                Destination::builder()
                    .to_addresses(payload.recipient)
                    .build(),
            )
            .content(content)
            .send()
            .await?;

        Ok(())
    }
}

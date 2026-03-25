use crate::service::{s3_adapter::S3Adapter, ses_adapter::SesAdapter};
use aws_sdk_s3::{
    error::SdkError as S3SdkError,
    operation::get_object::{GetObjectError, GetObjectOutput},
};
use aws_sdk_sesv2::{
    error::SdkError as SesSdkError,
    operation::send_email::{SendEmailError, SendEmailOutput},
    types::{EmailContent, MessageTag},
};
use serde_email::Email;

/// A no-op SES adapter used when email sending is not needed (e.g., the notification REST API).
pub struct NoopSesAdapter;

#[async_trait::async_trait]
impl SesAdapter for NoopSesAdapter {
    async fn send_email(
        &self,
        _from: Email,
        _to: Email,
        _content: EmailContent,
        _tags: Vec<MessageTag>,
    ) -> Result<SendEmailOutput, SesSdkError<SendEmailError>> {
        unimplemented!("NoopSesAdapter does not support send_email")
    }
}

/// A no-op S3 adapter used when S3 access is not needed (e.g., the notification REST API).
pub struct NoopS3Adapter;

#[async_trait::async_trait]
impl S3Adapter for NoopS3Adapter {
    async fn get_object(
        &self,
        _bucket: &str,
        _key: &str,
    ) -> Result<GetObjectOutput, S3SdkError<GetObjectError>> {
        unimplemented!("NoopS3Adapter does not support get_object")
    }
}

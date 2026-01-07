use aws_sdk_s3::{
    Client,
    error::SdkError,
    operation::get_object::{GetObjectError, GetObjectOutput},
};

#[async_trait::async_trait]
#[mockall::automock]
pub trait S3Adapter {
    async fn get_object(
        &self,
        bucket: &str,
        key: &str,
    ) -> Result<GetObjectOutput, SdkError<GetObjectError>>;
}

#[derive(Clone)]
pub struct S3AdapterImpl<'a> {
    client: &'a Client,
}

impl<'a> S3AdapterImpl<'a> {
    pub fn new(client: &'a Client) -> Self {
        Self { client }
    }
}

#[async_trait::async_trait]
impl<'a> S3Adapter for S3AdapterImpl<'a> {
    async fn get_object(
        &self,
        bucket: &str,
        key: &str,
    ) -> Result<GetObjectOutput, SdkError<GetObjectError>> {
        self.client
            .get_object()
            .bucket(bucket)
            .key(key)
            .send()
            .await
    }
}

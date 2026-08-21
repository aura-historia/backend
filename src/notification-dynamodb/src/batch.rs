use aws_sdk_dynamodb::types::{PutRequest, WriteRequest};
use serde::Serialize;

pub(crate) struct Batch<T, const N: usize>(Vec<T>);

impl<T, const N: usize> Batch<T, N> {
    pub(crate) fn chunked_from(iter: impl IntoIterator<Item = T>) -> Vec<Self> {
        let mut batches = Vec::new();
        let mut current = Vec::with_capacity(N);
        for value in iter {
            current.push(value);
            if current.len() == N {
                batches.push(Self(std::mem::take(&mut current)));
            }
        }
        if !current.is_empty() {
            batches.push(Self(current));
        }
        batches
    }
}

impl<T, const N: usize> std::ops::Deref for Batch<T, N> {
    type Target = [T];

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<T: Clone, const N: usize> Clone for Batch<T, N> {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}

impl<T: Serialize> Batch<T, 25> {
    pub(crate) fn into_dynamodb_write_requests(self) -> Vec<WriteRequest> {
        self.0
            .into_iter()
            .filter_map(|record| match serde_dynamo::to_item(record) {
                Ok(item) => {
                    let put_request = PutRequest::builder().set_item(Some(item)).build().ok()?;
                    Some(WriteRequest::builder().put_request(put_request).build())
                }
                Err(error) => {
                    tracing::warn!(error = %error, "Failed to serialize notification record.");
                    None
                }
            })
            .collect()
    }
}

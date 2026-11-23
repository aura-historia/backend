//! The few Cloud Storage JSON API calls the batch adapter stages its files with: upload, list and
//! download objects of one bucket.

use crate::EmbeddingError;
use url::Url;

const PRODUCTION_BASE_URL: &str = "https://storage.googleapis.com";
/// Objects one listing page returns at most.
const LIST_PAGE_SIZE: &str = "1000";

pub(crate) struct CloudStorageBucket {
    base_url: Url,
    bucket: String,
}

impl CloudStorageBucket {
    /// Fails closed on a name Cloud Storage would not accept, so it can never alter a request
    /// URL or a `gs://` URI.
    pub(crate) fn new(bucket: impl Into<String>) -> Result<Self, EmbeddingError> {
        let bucket = bucket.into();
        let valid = (3..=63).contains(&bucket.len())
            && bucket.bytes().all(|byte| {
                byte.is_ascii_lowercase() || byte.is_ascii_digit() || b"-_.".contains(&byte)
            })
            && bucket.starts_with(|c: char| c.is_ascii_alphanumeric())
            && bucket.ends_with(|c: char| c.is_ascii_alphanumeric());
        if !valid {
            return Err(EmbeddingError::InvalidInput {
                reason: "Cloud Storage bucket name is invalid",
            });
        }
        Ok(Self {
            base_url: Url::parse(PRODUCTION_BASE_URL).expect("the production URL is valid"),
            bucket,
        })
    }

    #[cfg(test)]
    pub(crate) fn with_base_url(mut self, base_url: Url) -> Self {
        self.base_url = base_url;
        self
    }

    pub(crate) fn uri(&self, object: &str) -> String {
        format!("gs://{}/{object}", self.bucket)
    }

    /// The object name of a `gs://` URI in this bucket.
    pub(crate) fn object_of<'a>(&self, uri: &'a str) -> Option<&'a str> {
        uri.strip_prefix("gs://")?
            .strip_prefix(self.bucket.as_str())?
            .strip_prefix('/')
    }

    pub(crate) async fn upload(
        &self,
        client: &reqwest::Client,
        token: &str,
        object: &str,
        content_type: &str,
        body: Vec<u8>,
    ) -> Result<(), EmbeddingError> {
        let mut url = self.url(&["upload", "storage", "v1", "b", &self.bucket, "o"]);
        url.query_pairs_mut()
            .append_pair("uploadType", "media")
            .append_pair("name", object);
        let response = client
            .post(url)
            .bearer_auth(token)
            .header(reqwest::header::CONTENT_TYPE, content_type)
            .body(body)
            .send()
            .await
            .map_err(|source| EmbeddingError::RequestFailed { source })?;
        success(response).map(drop)
    }

    /// Every object name under `prefix`.
    pub(crate) async fn list(
        &self,
        client: &reqwest::Client,
        token: &str,
        prefix: &str,
    ) -> Result<Vec<String>, EmbeddingError> {
        let mut names = Vec::new();
        let mut page_token = None::<String>;
        loop {
            let mut url = self.url(&["storage", "v1", "b", &self.bucket, "o"]);
            url.query_pairs_mut()
                .append_pair("prefix", prefix)
                .append_pair("maxResults", LIST_PAGE_SIZE)
                .append_pair("fields", "items(name),nextPageToken");
            if let Some(page_token) = &page_token {
                url.query_pairs_mut().append_pair("pageToken", page_token);
            }
            let page = success(
                client
                    .get(url)
                    .bearer_auth(token)
                    .send()
                    .await
                    .map_err(|source| EmbeddingError::RequestFailed { source })?,
            )?
            .json::<ObjectPage>()
            .await
            .map_err(|source| EmbeddingError::RequestFailed { source })?;
            names.extend(page.items.into_iter().map(|item| item.name));
            match page.next_page_token {
                Some(next) if !next.is_empty() => page_token = Some(next),
                _ => return Ok(names),
            }
        }
    }

    pub(crate) async fn download(
        &self,
        client: &reqwest::Client,
        token: &str,
        object: &str,
    ) -> Result<Vec<u8>, EmbeddingError> {
        let mut url = self.url(&["storage", "v1", "b", &self.bucket, "o", object]);
        url.query_pairs_mut().append_pair("alt", "media");
        let response = success(
            client
                .get(url)
                .bearer_auth(token)
                .send()
                .await
                .map_err(|source| EmbeddingError::RequestFailed { source })?,
        )?;
        response
            .bytes()
            .await
            .map(|bytes| bytes.to_vec())
            .map_err(|source| EmbeddingError::RequestFailed { source })
    }

    /// The base URL with `segments` appended, each percent-encoded as one path segment, so an
    /// object name's `/` stays part of the name.
    fn url(&self, segments: &[&str]) -> Url {
        let mut url = self.base_url.clone();
        url.path_segments_mut()
            .expect("an HTTP base URL has path segments")
            .pop_if_empty()
            .extend(segments);
        url
    }
}

fn success(response: reqwest::Response) -> Result<reqwest::Response, EmbeddingError> {
    if response.status().is_success() {
        Ok(response)
    } else {
        Err(EmbeddingError::ApiFailure {
            status: response.status(),
        })
    }
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct ObjectPage {
    #[serde(default)]
    items: Vec<ObjectItem>,
    next_page_token: Option<String>,
}

#[derive(serde::Deserialize)]
struct ObjectItem {
    name: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_reject_bucket_names_that_could_alter_a_uri() {
        for invalid in [
            "ab",
            "Bucket",
            "a/b",
            "bucket?",
            "-bucket",
            "bucket-",
            &"a".repeat(64),
        ] {
            assert!(CloudStorageBucket::new(invalid).is_err(), "{invalid}");
        }
        assert!(CloudStorageBucket::new("aura-embedding-batches").is_ok());
    }

    #[test]
    fn should_encode_object_names_as_one_path_segment() {
        let bucket = CloudStorageBucket::new("staging").unwrap();

        assert_eq!(
            "https://storage.googleapis.com/storage/v1/b/staging/o/batches%2Fa%2Fout.jsonl",
            bucket
                .url(&["storage", "v1", "b", "staging", "o", "batches/a/out.jsonl"])
                .as_str()
        );
    }

    #[test]
    fn should_map_only_uris_of_its_own_bucket_to_object_names() {
        let bucket = CloudStorageBucket::new("staging").unwrap();

        assert_eq!(Some("a/b"), bucket.object_of(&bucket.uri("a/b")));
        assert_eq!(None, bucket.object_of("gs://staging-other/a/b"));
        assert_eq!(None, bucket.object_of("https://staging/a/b"));
    }
}

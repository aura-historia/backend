//! Vertex AI batch prediction for product embeddings, at half the synchronous price.
//!
//! A batch stages under `embedding-batches/<label>/` in a Cloud Storage bucket: each fetched
//! image as an object of its own, referenced by `gs://` URI because a batch cannot fetch HTTP
//! URLs; the JSONL requests; and the job's output. The bucket's lifecycle rule deletes the
//! staged objects ([durable worker runbook](../../../docs/durable-worker-runbook.md)).
//!
//! Requests carry the same text and the same image bytes as the synchronous
//! [`VertexAiEmbeddingGenerator`](crate::VertexAiEmbeddingGenerator), with no preprocessing on
//! either path, so both embed a listing identically.

use crate::{
    EMBEDDING_DIMENSIONS, EmbeddingBatchId, EmbeddingBatchPoll, EmbeddingError, EmbeddingVector,
    EndedEmbeddingBatch, ProductEmbeddingBatches, ProductEmbeddingRequest, UnansweredEmbeddings,
    VertexAiEmbeddingConfig,
    cloud_storage::CloudStorageBucket,
    vertex::{
        CONNECT_TIMEOUT, EmbedContentResponse, GEMINI_EMBEDDING_MODEL_RESOURCE, api_endpoint,
        fetch_image, location_resource, product_text,
    },
};
use base64::{Engine, engine::general_purpose::STANDARD as BASE64};
use futures::{StreamExt, TryStreamExt, stream};
use google_cloud_auth::credentials::AccessTokenCredentials;
use image_fetcher::ImageFetcher;
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, time::Duration};

/// Locations that serve `gemini-embedding-2` batch prediction.
const BATCH_LOCATIONS: [&str; 3] = ["global", "us", "eu"];
const STAGING_ROOT: &str = "embedding-batches";
/// Images fetched and staged at once while a batch is submitted.
const STAGING_CONCURRENCY: usize = 32;
/// Bounds one provider exchange, including an upload or an output download.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(120);
const CREDENTIAL_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VertexAiBatchConfig {
    embedding: VertexAiEmbeddingConfig,
    staging_bucket: String,
}

impl VertexAiBatchConfig {
    pub fn new(embedding: VertexAiEmbeddingConfig, staging_bucket: impl Into<String>) -> Self {
        Self {
            embedding,
            staging_bucket: staging_bucket.into(),
        }
    }
}

/// Vertex AI batch prediction implementation of [`ProductEmbeddingBatches`].
pub struct VertexAiProductEmbeddingBatches {
    api_base_url: String,
    jobs_url: String,
    client: reqwest::Client,
    image_fetcher: ImageFetcher,
    credentials: AccessTokenCredentials,
    bucket: CloudStorageBucket,
}

impl VertexAiProductEmbeddingBatches {
    /// Fails on a location without `gemini-embedding-2` batch prediction or an invalid bucket.
    pub fn new(
        config: VertexAiBatchConfig,
        credentials: AccessTokenCredentials,
    ) -> Result<Self, EmbeddingError> {
        let location = config.embedding.location();
        if !BATCH_LOCATIONS.contains(&location) {
            return Err(EmbeddingError::InvalidInput {
                reason: "Vertex AI location has no gemini-embedding-2 batch prediction",
            });
        }
        let api_base_url = format!("{}/v1", api_endpoint(&config.embedding));
        let client = reqwest::Client::builder()
            .connect_timeout(CONNECT_TIMEOUT)
            .timeout(REQUEST_TIMEOUT)
            .build()
            .map_err(|source| EmbeddingError::ClientInitializationFailed { source })?;
        Ok(Self {
            jobs_url: format!(
                "{api_base_url}/{}/batchPredictionJobs",
                location_resource(&config.embedding)
            ),
            api_base_url,
            client,
            image_fetcher: ImageFetcher::new(),
            credentials,
            bucket: CloudStorageBucket::new(config.staging_bucket)?,
        })
    }

    async fn access_token(&self) -> Result<String, EmbeddingError> {
        tokio::time::timeout(CREDENTIAL_TIMEOUT, self.credentials.access_token())
            .await
            .map_err(|_| EmbeddingError::DeadlineExceeded {
                deadline: CREDENTIAL_TIMEOUT,
            })?
            .map(|token| token.token)
            .map_err(|source| EmbeddingError::AuthenticationFailed {
                source: Box::new(source),
            })
    }

    /// Fetches the request's image and stages it; one that cannot be fetched is left out, as on
    /// the synchronous path.
    async fn batch_line(
        &self,
        token: &str,
        prefix: &str,
        request: &ProductEmbeddingRequest,
    ) -> Result<String, EmbeddingError> {
        let mut parts = vec![BatchPart::Text {
            text: product_text(&request.title, request.additional_text.as_ref()),
        }];
        let image = match &request.image_url {
            Some(url) => fetch_image(&self.image_fetcher, url).await,
            None => None,
        };
        if let Some(image) = image {
            let bytes =
                BASE64
                    .decode(image.base64_data())
                    .map_err(|_| EmbeddingError::InvalidInput {
                        reason: "fetched embedding image is not valid base64",
                    })?;
            let object = format!("{prefix}/images/{}", request.key);
            self.bucket
                .upload(&self.client, token, &object, image.mime_type(), bytes)
                .await?;
            parts.push(BatchPart::FileData {
                file_data: FileData {
                    file_uri: self.bucket.uri(&object),
                    mime_type: image.mime_type(),
                },
            });
        }
        serde_json::to_string(&BatchLine {
            key: request.key.to_string(),
            request: BatchRequest {
                content: BatchContent { parts },
                embed_content_config: EmbedContentConfig {
                    output_dimensionality: EMBEDDING_DIMENSIONS,
                },
            },
        })
        .map_err(|_| EmbeddingError::InvalidInput {
            reason: "embedding batch request cannot be serialized",
        })
    }

    async fn create_job(
        &self,
        token: &str,
        label: &str,
        prefix: &str,
    ) -> Result<EmbeddingBatchId, EmbeddingError> {
        let body = CreateJob {
            display_name: format!("product-embeddings-{label}"),
            model: GEMINI_EMBEDDING_MODEL_RESOURCE,
            input_config: InputConfig {
                instances_format: "jsonl",
                gcs_source: GcsSource {
                    uris: vec![self.bucket.uri(&format!("{prefix}/input.jsonl"))],
                },
            },
            output_config: OutputConfig {
                predictions_format: "jsonl",
                gcs_destination: GcsDestination {
                    output_uri_prefix: self.bucket.uri(&format!("{prefix}/output")),
                },
            },
        };
        let response = self
            .client
            .post(&self.jobs_url)
            .bearer_auth(token)
            .json(&body)
            .send()
            .await
            .map_err(|source| EmbeddingError::RequestFailed { source })?;
        if !response.status().is_success() {
            return Err(EmbeddingError::ApiFailure {
                status: response.status(),
            });
        }
        let job = response
            .json::<CreatedJob>()
            .await
            .map_err(|source| EmbeddingError::RequestFailed { source })?;
        job_id(job.name)
    }

    async fn get_job(&self, token: &str, batch: &EmbeddingBatchId) -> Result<Job, EmbeddingError> {
        let response = self
            .client
            .get(format!("{}/{}", self.api_base_url, batch.as_str()))
            .bearer_auth(token)
            .send()
            .await
            .map_err(|source| EmbeddingError::RequestFailed { source })?;
        if !response.status().is_success() {
            return Err(EmbeddingError::ApiFailure {
                status: response.status(),
            });
        }
        response
            .json::<Job>()
            .await
            .map_err(|source| EmbeddingError::RequestFailed { source })
    }

    /// Every result line the job wrote, from each JSONL file under its output prefix.
    async fn read_results(
        &self,
        token: &str,
        job: &Job,
    ) -> Result<Vec<(u32, Result<EmbeddingVector, EmbeddingError>)>, EmbeddingError> {
        let output_prefix = self
            .bucket
            .object_of(&job.output_config.gcs_destination.output_uri_prefix)
            .ok_or(EmbeddingError::InvalidResponse {
                reason: "batch output is outside the staging bucket",
            })?;
        let mut results = BTreeMap::new();
        for object in self
            .bucket
            .list(&self.client, token, &format!("{output_prefix}/"))
            .await?
        {
            if !object.ends_with(".jsonl") {
                continue;
            }
            let file = self.bucket.download(&self.client, token, &object).await?;
            for (key, result) in parse_output(&file) {
                results.entry(key).or_insert(result);
            }
        }
        Ok(results.into_iter().collect())
    }
}

#[async_trait::async_trait]
impl ProductEmbeddingBatches for VertexAiProductEmbeddingBatches {
    async fn submit(
        &self,
        label: &str,
        requests: &[ProductEmbeddingRequest],
    ) -> Result<EmbeddingBatchId, EmbeddingError> {
        if label.is_empty()
            || !label
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        {
            return Err(EmbeddingError::InvalidInput {
                reason: "embedding batch label is invalid",
            });
        }
        let prefix = format!("{STAGING_ROOT}/{label}");
        let token = self.access_token().await?;
        // Collected first: a lazily mapped stream would not prove itself `Send` here.
        let staging = requests
            .iter()
            .map(|request| self.batch_line(&token, &prefix, request))
            .collect::<Vec<_>>();
        let lines = stream::iter(staging)
            .buffered(STAGING_CONCURRENCY)
            .try_collect::<Vec<_>>()
            .await?;
        let mut input = lines.join("\n");
        input.push('\n');
        self.bucket
            .upload(
                &self.client,
                &token,
                &format!("{prefix}/input.jsonl"),
                "application/jsonl",
                input.into_bytes(),
            )
            .await?;
        self.create_job(&token, label, &prefix).await
    }

    async fn poll(&self, batch: &EmbeddingBatchId) -> Result<EmbeddingBatchPoll, EmbeddingError> {
        let token = self.access_token().await?;
        let job = self.get_job(&token, batch).await?;
        let unanswered = match JobState::of(&job.state) {
            JobState::Running => return Ok(EmbeddingBatchPoll::InProgress),
            JobState::Succeeded => UnansweredEmbeddings::Failed,
            JobState::Failed | JobState::Stopped => UnansweredEmbeddings::NotProcessed,
        };
        Ok(EmbeddingBatchPoll::Ended(EndedEmbeddingBatch {
            results: self.read_results(&token, &job).await?,
            unanswered,
        }))
    }
}

/// The job states, by what they mean for its unanswered requests.
#[derive(Debug, PartialEq, Eq)]
enum JobState {
    /// Not ended, as far as this adapter knows. A paused job resumes; a state it does not know
    /// is waited out rather than closed, because closing a job Vertex AI still runs would pay
    /// for its work twice.
    Running,
    /// Ran every request; a request missing from the output failed on its own.
    Succeeded,
    /// The job as a whole failed, such as for a missing permission or quota. That says nothing
    /// about its documents.
    Failed,
    /// Canceled or expired before it reached every request.
    Stopped,
}

impl JobState {
    fn of(state: &str) -> Self {
        match state {
            "JOB_STATE_SUCCEEDED" | "JOB_STATE_PARTIALLY_SUCCEEDED" => Self::Succeeded,
            "JOB_STATE_FAILED" => Self::Failed,
            "JOB_STATE_CANCELLED" | "JOB_STATE_EXPIRED" => Self::Stopped,
            _ => Self::Running,
        }
    }
}

fn job_id(name: String) -> Result<EmbeddingBatchId, EmbeddingError> {
    if !name.starts_with("projects/") || !name.contains("/batchPredictionJobs/") {
        return Err(EmbeddingError::InvalidResponse {
            reason: "batch job name is invalid",
        });
    }
    EmbeddingBatchId::new(name).map_err(|_| EmbeddingError::InvalidResponse {
        reason: "batch job name is invalid",
    })
}

/// The results of one output file. A line without a valid key cannot be attributed and is
/// skipped, so its request counts as unanswered; a line with a status is a failed request.
fn parse_output(file: &[u8]) -> Vec<(u32, Result<EmbeddingVector, EmbeddingError>)> {
    file.split(|byte| *byte == b'\n')
        .filter(|line| !line.iter().all(u8::is_ascii_whitespace))
        .filter_map(|line| {
            let line = serde_json::from_slice::<OutputLine>(line).ok()?;
            let key = line.key?.parse::<u32>().ok()?;
            let result = match (
                line.status.filter(|status| !status.is_empty()),
                line.response,
            ) {
                (None, Some(response)) => serde_json::from_value::<EmbedContentResponse>(response)
                    .map_err(|_| EmbeddingError::InvalidResponse {
                        reason: "batch embedding response is invalid",
                    })
                    .and_then(EmbedContentResponse::into_embedding_vector),
                (Some(_), _) => Err(EmbeddingError::InvalidResponse {
                    reason: "batch request failed at the provider",
                }),
                (None, None) => Err(EmbeddingError::InvalidResponse {
                    reason: "batch embedding response is missing",
                }),
            };
            Some((key, result))
        })
        .collect()
}

#[derive(Serialize)]
struct BatchLine {
    key: String,
    request: BatchRequest,
}

#[derive(Serialize)]
struct BatchRequest {
    content: BatchContent,
    embed_content_config: EmbedContentConfig,
}

#[derive(Serialize)]
struct BatchContent {
    parts: Vec<BatchPart>,
}

#[derive(Serialize)]
#[serde(untagged)]
enum BatchPart {
    Text {
        text: String,
    },
    FileData {
        #[serde(rename = "fileData")]
        file_data: FileData,
    },
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct FileData {
    file_uri: String,
    mime_type: &'static str,
}

#[derive(Serialize)]
struct EmbedContentConfig {
    output_dimensionality: usize,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct CreateJob {
    display_name: String,
    model: &'static str,
    input_config: InputConfig,
    output_config: OutputConfig,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct InputConfig {
    instances_format: &'static str,
    gcs_source: GcsSource,
}

#[derive(Serialize)]
struct GcsSource {
    uris: Vec<String>,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct OutputConfig {
    #[serde(skip_deserializing, default)]
    predictions_format: &'static str,
    gcs_destination: GcsDestination,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GcsDestination {
    output_uri_prefix: String,
}

#[derive(Deserialize)]
struct CreatedJob {
    name: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Job {
    state: String,
    output_config: OutputConfig,
}

#[derive(Deserialize)]
struct OutputLine {
    key: Option<String>,
    response: Option<serde_json::Value>,
    status: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{EmbeddingText, test_support::TestCredentials};
    use serde_json::{Value, json};
    use wiremock::{
        Mock, MockServer, Request, ResponseTemplate,
        matchers::{method, path, query_param},
    };

    const JOB: &str = "projects/test-project/locations/eu/batchPredictionJobs/42";

    async fn batches(
        server: &MockServer,
    ) -> Result<VertexAiProductEmbeddingBatches, Box<dyn std::error::Error>> {
        let mut batches = VertexAiProductEmbeddingBatches::new(
            VertexAiBatchConfig::new(
                VertexAiEmbeddingConfig::new("test-project", "eu"),
                "staging",
            ),
            AccessTokenCredentials::from(TestCredentials::Issued),
        )?;
        // Reach the loopback fixture even when the developer environment configures a proxy.
        batches.client = reqwest::Client::builder().no_proxy().build()?;
        batches.api_base_url = format!("{}/v1", server.uri());
        batches.jobs_url = format!(
            "{}/v1/projects/test-project/locations/eu/batchPredictionJobs",
            server.uri()
        );
        batches.bucket = CloudStorageBucket::new("staging")?.with_base_url(server.uri().parse()?);
        Ok(batches)
    }

    fn request(key: u32, additional_text: Option<&str>) -> ProductEmbeddingRequest {
        ProductEmbeddingRequest {
            key,
            title: EmbeddingText::new("vintage brass lamp").unwrap(),
            additional_text: additional_text.map(|text| EmbeddingText::new(text).unwrap()),
            image_url: None,
        }
    }

    fn body_json(request: &Request) -> Value {
        serde_json::from_slice(&request.body).unwrap()
    }

    #[tokio::test]
    async fn should_stage_jsonl_requests_and_create_a_job_reading_them()
    -> Result<(), Box<dyn std::error::Error>> {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/upload/storage/v1/b/staging/o"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({})))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path(
                "/v1/projects/test-project/locations/eu/batchPredictionJobs",
            ))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(json!({"name": JOB, "state": "JOB_STATE_PENDING"})),
            )
            .mount(&server)
            .await;
        let batches = batches(&server).await?;

        let id = batches
            .submit(
                "0b7e-label",
                &[request(1, Some("adjustable arm")), request(2, None)],
            )
            .await?;

        assert_eq!(JOB, id.as_str());
        let received = server.received_requests().await.unwrap();
        let upload = &received[0];
        assert_eq!(
            Some("embedding-batches/0b7e-label/input.jsonl"),
            upload
                .url
                .query_pairs()
                .find(|(name, _)| name == "name")
                .map(|(_, value)| value.into_owned())
                .as_deref()
        );
        let lines = std::str::from_utf8(&upload.body)?
            .lines()
            .map(serde_json::from_str::<Value>)
            .collect::<Result<Vec<_>, _>>()?;
        assert_eq!(
            vec![
                json!({"key": "1", "request": {
                    "content": {"parts": [{"text": "title: vintage brass lamp | text: adjustable arm"}]},
                    "embed_content_config": {"output_dimensionality": 768}}}),
                json!({"key": "2", "request": {
                    "content": {"parts": [{"text": "title: vintage brass lamp | text: none"}]},
                    "embed_content_config": {"output_dimensionality": 768}}}),
            ],
            lines
        );
        assert_eq!(
            json!({
                "displayName": "product-embeddings-0b7e-label",
                "model": "publishers/google/models/gemini-embedding-2",
                "inputConfig": {"instancesFormat": "jsonl", "gcsSource": {
                    "uris": ["gs://staging/embedding-batches/0b7e-label/input.jsonl"]}},
                "outputConfig": {"predictionsFormat": "jsonl", "gcsDestination": {
                    "outputUriPrefix": "gs://staging/embedding-batches/0b7e-label/output"}},
            }),
            body_json(&received[1])
        );
        Ok(())
    }

    #[test]
    fn should_reference_a_staged_image_by_its_cloud_storage_uri() {
        let line = BatchPart::FileData {
            file_data: FileData {
                file_uri: "gs://staging/embedding-batches/a/images/7".to_owned(),
                mime_type: "image/png",
            },
        };

        assert_eq!(
            json!({"fileData": {"fileUri": "gs://staging/embedding-batches/a/images/7",
                "mimeType": "image/png"}}),
            serde_json::to_value(line).unwrap()
        );
    }

    #[tokio::test]
    async fn should_reject_labels_that_could_escape_the_staging_prefix()
    -> Result<(), Box<dyn std::error::Error>> {
        let server = MockServer::start().await;
        let batches = batches(&server).await?;

        for label in ["", "../other", "Label", "a/b"] {
            assert!(
                matches!(
                    batches.submit(label, &[request(1, None)]).await,
                    Err(EmbeddingError::InvalidInput { .. })
                ),
                "{label}"
            );
        }
        assert!(server.received_requests().await.unwrap().is_empty());
        Ok(())
    }

    #[tokio::test]
    async fn should_report_a_running_job_as_in_progress() -> Result<(), Box<dyn std::error::Error>>
    {
        let server = MockServer::start().await;
        mount_job(&server, "JOB_STATE_RUNNING").await;
        let batches = batches(&server).await?;

        let poll = batches.poll(&EmbeddingBatchId::new(JOB)?).await?;

        assert!(matches!(poll, EmbeddingBatchPoll::InProgress));
        Ok(())
    }

    #[tokio::test]
    async fn should_read_every_output_file_and_attribute_results_by_key()
    -> Result<(), Box<dyn std::error::Error>> {
        let server = MockServer::start().await;
        mount_job(&server, "JOB_STATE_PARTIALLY_SUCCEEDED").await;
        Mock::given(method("GET"))
            .and(path("/storage/v1/b/staging/o"))
            .and(query_param("prefix", "embedding-batches/a/output/"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"items": [
                {"name": "embedding-batches/a/output/prediction-model-1/predictions.jsonl"},
                {"name": "embedding-batches/a/output/prediction-model-1/README"},
            ]})))
            .mount(&server)
            .await;
        let values = vec![2.0_f32; EMBEDDING_DIMENSIONS];
        let output = [
            json!({"key": "1", "request": {}, "response": {"embedding": {"values": values}}})
                .to_string(),
            json!({"key": "2", "request": {}, "response": {}, "status": "Bad Request: SENTINEL"})
                .to_string(),
            json!({"key": "3", "response": {"embedding": {"values": [1.0]}}}).to_string(),
            json!({"key": "not-a-key", "response": {}}).to_string(),
            "not json".to_owned(),
        ]
        .join("\n");
        Mock::given(method("GET"))
            .and(path(
                "/storage/v1/b/staging/o/embedding-batches%2Fa%2Foutput%2Fprediction-model-1%2Fpredictions.jsonl",
            ))
            .and(query_param("alt", "media"))
            .respond_with(ResponseTemplate::new(200).set_body_string(output))
            .mount(&server)
            .await;
        let batches = batches(&server).await?;

        let EmbeddingBatchPoll::Ended(ended) = batches.poll(&EmbeddingBatchId::new(JOB)?).await?
        else {
            panic!("the job ended");
        };

        assert_eq!(UnansweredEmbeddings::Failed, ended.unanswered);
        let keys = ended
            .results
            .iter()
            .map(|(key, result)| (*key, result.is_ok()))
            .collect::<Vec<_>>();
        assert_eq!(vec![(1, true), (2, false), (3, false)], keys);
        Ok(())
    }

    #[tokio::test]
    async fn should_leave_requests_of_a_canceled_job_unprocessed()
    -> Result<(), Box<dyn std::error::Error>> {
        let server = MockServer::start().await;
        mount_job(&server, "JOB_STATE_CANCELLED").await;
        Mock::given(method("GET"))
            .and(path("/storage/v1/b/staging/o"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({})))
            .mount(&server)
            .await;
        let batches = batches(&server).await?;

        let EmbeddingBatchPoll::Ended(ended) = batches.poll(&EmbeddingBatchId::new(JOB)?).await?
        else {
            panic!("the job ended");
        };

        assert_eq!(UnansweredEmbeddings::NotProcessed, ended.unanswered);
        assert!(ended.results.is_empty());
        Ok(())
    }

    #[tokio::test]
    async fn should_refuse_output_outside_the_staging_bucket()
    -> Result<(), Box<dyn std::error::Error>> {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path(format!("/v1/{JOB}")))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "state": "JOB_STATE_SUCCEEDED",
                "outputConfig": {"gcsDestination": {"outputUriPrefix": "gs://elsewhere/out"}},
            })))
            .mount(&server)
            .await;
        let batches = batches(&server).await?;

        assert!(matches!(
            batches.poll(&EmbeddingBatchId::new(JOB)?).await,
            Err(EmbeddingError::InvalidResponse { .. })
        ));
        Ok(())
    }

    #[test]
    fn should_classify_job_states_and_wait_out_unknown_ones() {
        assert_eq!(JobState::Running, JobState::of("JOB_STATE_PAUSED"));
        assert_eq!(
            JobState::Succeeded,
            JobState::of("JOB_STATE_PARTIALLY_SUCCEEDED")
        );
        assert_eq!(JobState::Failed, JobState::of("JOB_STATE_FAILED"));
        assert_eq!(JobState::Stopped, JobState::of("JOB_STATE_EXPIRED"));
        assert_eq!(JobState::Running, JobState::of("JOB_STATE_UNSPECIFIED"));
    }

    #[test]
    fn should_address_the_regional_endpoint_of_a_multi_region() {
        let batches = VertexAiProductEmbeddingBatches::new(
            VertexAiBatchConfig::new(
                VertexAiEmbeddingConfig::new("test-project", "eu"),
                "staging",
            ),
            AccessTokenCredentials::from(TestCredentials::Issued),
        )
        .unwrap();

        assert_eq!(
            "https://aiplatform.eu.rep.googleapis.com/v1/projects/test-project/locations/eu/batchPredictionJobs",
            batches.jobs_url
        );
    }

    #[test]
    fn should_refuse_locations_without_batch_prediction() {
        assert!(matches!(
            VertexAiProductEmbeddingBatches::new(
                VertexAiBatchConfig::new(
                    VertexAiEmbeddingConfig::new("test-project", "europe-west4"),
                    "staging",
                ),
                AccessTokenCredentials::from(TestCredentials::Issued),
            ),
            Err(EmbeddingError::InvalidInput { .. })
        ));
    }

    async fn mount_job(server: &MockServer, state: &str) {
        Mock::given(method("GET"))
            .and(path(format!("/v1/{JOB}")))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "name": JOB,
                "state": state,
                "outputConfig": {"gcsDestination": {
                    "outputUriPrefix": "gs://staging/embedding-batches/a/output"}},
            })))
            .mount(server)
            .await;
    }
}

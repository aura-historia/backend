//! Provider fakes for the title translation and product embedding rounds.

#![allow(dead_code)]

use large_language_model::{
    BatchItemOutcome, BatchItemResult, BatchProcessingStatus, BatchRequest, LargeLanguageModel,
    LargeLanguageModelBatchId, LargeLanguageModelBatches, LargeLanguageModelError, LlmOperation,
    StructuredGenerationRequest,
};
use serde::de::DeserializeOwned;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};

/// Answers every title of every request with `translation(language)`, from the title IDs and
/// target languages the request's schema requires.
fn answer(
    request: &StructuredGenerationRequest,
    translation: fn(&str) -> String,
) -> serde_json::Value {
    let schema = &request.response_json_schema;
    let languages = schema["$defs"]["title"]["required"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let titles = schema["properties"]["translations"]["required"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .map(|key| {
            let entry = languages
                .iter()
                .map(|language| {
                    let language = language.as_str().unwrap_or_default();
                    (
                        language.to_owned(),
                        serde_json::Value::from(translation(language)),
                    )
                })
                .collect::<serde_json::Map<_, _>>();
            (key.as_str().unwrap_or_default().to_owned(), entry.into())
        })
        .collect::<serde_json::Map<_, _>>();
    serde_json::json!({ "translations": titles })
}

/// A provider batch that ends once `ended` is set and translates every title, except that it
/// refuses every request whose prompt contains `refuse`.
#[derive(Clone)]
pub struct FakeBatches {
    pub ended: Arc<AtomicBool>,
    pub submitted: Arc<Mutex<Vec<Vec<BatchRequest>>>>,
    pub translation: fn(&str) -> String,
    pub refuse: Option<&'static str>,
}

impl FakeBatches {
    pub fn new(translation: fn(&str) -> String) -> Self {
        Self {
            ended: Arc::new(AtomicBool::new(true)),
            submitted: Arc::default(),
            translation,
            refuse: None,
        }
    }

    pub fn submissions(&self) -> Vec<Vec<BatchRequest>> {
        self.submitted.lock().unwrap().clone()
    }
}

#[async_trait::async_trait]
impl LargeLanguageModelBatches for FakeBatches {
    async fn submit_batch(
        &self,
        _: LlmOperation,
        requests: Vec<BatchRequest>,
    ) -> Result<LargeLanguageModelBatchId, LargeLanguageModelError> {
        let mut submitted = self.submitted.lock().unwrap();
        submitted.push(requests);
        LargeLanguageModelBatchId::new(format!("msgbatch_{}", submitted.len()))
    }

    async fn batch_status(
        &self,
        _: &LargeLanguageModelBatchId,
    ) -> Result<BatchProcessingStatus, LargeLanguageModelError> {
        Ok(if self.ended.load(Ordering::SeqCst) {
            BatchProcessingStatus::Ended
        } else {
            BatchProcessingStatus::InProgress
        })
    }

    async fn batch_results<Output>(
        &self,
        _: LlmOperation,
        batch: &LargeLanguageModelBatchId,
    ) -> Result<Vec<BatchItemResult<Output>>, LargeLanguageModelError>
    where
        Output: DeserializeOwned + Send,
    {
        let index = batch
            .as_str()
            .trim_start_matches("msgbatch_")
            .parse::<usize>()
            .expect("a batch this fake created");
        let requests = self.submitted.lock().unwrap()[index - 1].clone();
        Ok(requests
            .into_iter()
            .map(|BatchRequest { custom_id, request }| BatchItemResult {
                custom_id: custom_id.as_str().to_owned(),
                outcome: if self
                    .refuse
                    .is_some_and(|text| request.prompt.contains(text))
                {
                    BatchItemOutcome::Failed(LargeLanguageModelError::refusal())
                } else {
                    BatchItemOutcome::Succeeded(
                        serde_json::from_value(answer(&request, self.translation)).unwrap(),
                    )
                },
            })
            .collect())
    }
}

/// The refusal fallback model: translates every title and counts its calls.
#[derive(Clone)]
pub struct FakeFallback {
    pub calls: Arc<AtomicUsize>,
    pub translation: fn(&str) -> String,
}

impl FakeFallback {
    pub fn new(translation: fn(&str) -> String) -> Self {
        Self {
            calls: Arc::default(),
            translation,
        }
    }
}

#[async_trait::async_trait]
impl LargeLanguageModel for FakeFallback {
    async fn generate<Output>(
        &self,
        request: StructuredGenerationRequest,
    ) -> Result<Output, LargeLanguageModelError>
    where
        Output: DeserializeOwned + Send,
    {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(serde_json::from_value(answer(&request, self.translation)).unwrap())
    }
}

/// Answers every request of a batch with a unit vector once it is polled, unless told to leave
/// the next poll's requests unprocessed. Records what each submission requested.
#[derive(Clone, Default)]
pub struct FakeEmbeddingBatches {
    pub submitted: Arc<Mutex<Vec<Vec<embedding::ProductEmbeddingRequest>>>>,
    pub leave_unprocessed: Arc<AtomicBool>,
}

#[async_trait::async_trait]
impl embedding::ProductEmbeddingBatches for FakeEmbeddingBatches {
    async fn submit(
        &self,
        _: &str,
        requests: &[embedding::ProductEmbeddingRequest],
    ) -> Result<embedding::EmbeddingBatchId, embedding::EmbeddingError> {
        let mut submitted = self.submitted.lock().unwrap();
        submitted.push(requests.to_vec());
        embedding::EmbeddingBatchId::new(format!(
            "projects/p/locations/eu/batchPredictionJobs/{}",
            submitted.len()
        ))
    }

    async fn poll(
        &self,
        batch: &embedding::EmbeddingBatchId,
    ) -> Result<embedding::EmbeddingBatchPoll, embedding::EmbeddingError> {
        use embedding::{EmbeddingBatchPoll, EndedEmbeddingBatch, UnansweredEmbeddings};
        if self.leave_unprocessed.swap(false, Ordering::SeqCst) {
            return Ok(EmbeddingBatchPoll::Ended(EndedEmbeddingBatch {
                results: Vec::new(),
                unanswered: UnansweredEmbeddings::NotProcessed,
            }));
        }
        let index = batch
            .as_str()
            .rsplit('/')
            .next()
            .and_then(|number| number.parse::<usize>().ok())
            .unwrap();
        let requests = self.submitted.lock().unwrap()[index - 1].clone();
        Ok(EmbeddingBatchPoll::Ended(EndedEmbeddingBatch {
            results: requests
                .iter()
                .map(|request| {
                    (
                        request.key,
                        embedding::EmbeddingVector::try_new(vec![
                            1.0;
                            embedding::EMBEDDING_DIMENSIONS
                        ]),
                    )
                })
                .collect(),
            unanswered: UnansweredEmbeddings::Failed,
        }))
    }
}

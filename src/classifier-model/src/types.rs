use crate::ClassificationError;
use futures::StreamExt;
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, num::NonZeroUsize, sync::Arc, time::Duration};
use url::Url;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClassificationOperation {
    ProductEnhancedSearchDescriptionMatching,
}

impl ClassificationOperation {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ProductEnhancedSearchDescriptionMatching => {
                "product_enhanced_search_description_matching"
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct QuestionId(String);

impl QuestionId {
    pub fn new(value: impl Into<String>) -> Result<Self, ClassificationError> {
        let value = value.into();
        if value.is_empty()
            || value.len() > 100
            || !value
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"_.-".contains(&byte))
        {
            return Err(ClassificationError::InvalidRequest);
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BinaryClassificationQuestion {
    pub id: QuestionId,
    pub instructions: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClassificationOptions {
    pub request_timeout: Duration,
}

impl Default for ClassificationOptions {
    fn default() -> Self {
        Self {
            request_timeout: Duration::from_secs(30),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ClassificationRequest {
    pub operation: ClassificationOperation,
    pub state: serde_json::Value,
    pub questions: Vec<BinaryClassificationQuestion>,
    pub image_urls: Vec<Url>,
    pub options: ClassificationOptions,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClassificationBatchOptions {
    pub max_concurrent_requests: NonZeroUsize,
}

impl ClassificationBatchOptions {
    pub fn new(max_concurrent_requests: NonZeroUsize) -> Self {
        Self {
            max_concurrent_requests,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Probability(f64);

impl Eq for Probability {}

impl Probability {
    pub fn new(value: f64) -> Result<Self, ClassificationError> {
        if value.is_finite() && (0.0..=1.0).contains(&value) {
            Ok(Self(value))
        } else {
            Err(ClassificationError::InvalidResponse)
        }
    }

    pub fn get(self) -> f64 {
        self.0
    }
}

impl Serialize for Probability {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_f64(self.0)
    }
}

impl<'de> Deserialize<'de> for Probability {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = f64::deserialize(deserializer)?;
        Self::new(value).map_err(serde::de::Error::custom)
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClassificationUsage {
    #[serde(default)]
    pub input_tokens: Option<u64>,
    #[serde(default)]
    pub output_tokens: Option<u64>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ClassificationResponse {
    pub answers: BTreeMap<QuestionId, Probability>,
    pub usage: ClassificationUsage,
    pub diagnostics: ClassificationDiagnostics,
}

/// Bounded, provider-neutral attempt metadata safe for structured operational logging.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ClassificationDiagnostics {
    pub provider: String,
    pub model: String,
    pub duration_millis: u64,
    pub requested_image_count: usize,
    pub sent_image_count: usize,
    pub image_omission_reason: Option<ImageOmissionReason>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageOmissionReason {
    FetchFailed,
    UnsupportedFormat,
    InvalidOrOversized,
}

impl ImageOmissionReason {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::FetchFailed => "fetch_failed",
            Self::UnsupportedFormat => "unsupported_format",
            Self::InvalidOrOversized => "invalid_or_oversized",
        }
    }
}

/// A provider-neutral classification capability. The default batch method preserves input order,
/// returns one result per input, and polls at most the requested number of requests concurrently.
/// It does not spawn detached tasks: dropping the batch future drops its in-flight request futures.
#[async_trait::async_trait]
pub trait ClassifierModel: Send + Sync {
    /// Stable, non-secret classifier identity for operational diagnostics.
    fn provider_name(&self) -> &'static str {
        "unknown"
    }

    /// Stable configured model identifier for operational diagnostics.
    fn model_name(&self) -> &'static str {
        "unknown"
    }

    async fn classify(
        &self,
        request: ClassificationRequest,
    ) -> Result<ClassificationResponse, ClassificationError>;

    async fn classify_batch(
        &self,
        requests: Vec<ClassificationRequest>,
        options: ClassificationBatchOptions,
    ) -> Vec<Result<ClassificationResponse, ClassificationError>> {
        futures::stream::iter(requests)
            .map(|request| self.classify(request))
            .buffered(options.max_concurrent_requests.get())
            .collect()
            .await
    }
}

#[async_trait::async_trait]
impl<M> ClassifierModel for Arc<M>
where
    M: ClassifierModel + ?Sized,
{
    fn provider_name(&self) -> &'static str {
        self.as_ref().provider_name()
    }

    fn model_name(&self) -> &'static str {
        self.as_ref().model_name()
    }

    async fn classify(
        &self,
        request: ClassificationRequest,
    ) -> Result<ClassificationResponse, ClassificationError> {
        self.as_ref().classify(request).await
    }

    async fn classify_batch(
        &self,
        requests: Vec<ClassificationRequest>,
        options: ClassificationBatchOptions,
    ) -> Vec<Result<ClassificationResponse, ClassificationError>> {
        self.as_ref().classify_batch(requests, options).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tokio::time::{Duration, sleep};

    fn request() -> ClassificationRequest {
        ClassificationRequest {
            operation: ClassificationOperation::ProductEnhancedSearchDescriptionMatching,
            state: serde_json::json!({}),
            questions: Vec::new(),
            image_urls: Vec::new(),
            options: ClassificationOptions::default(),
        }
    }

    struct BatchModel {
        active: AtomicUsize,
        max_active: AtomicUsize,
    }

    #[async_trait::async_trait]
    impl ClassifierModel for BatchModel {
        async fn classify(
            &self,
            request: ClassificationRequest,
        ) -> Result<ClassificationResponse, ClassificationError> {
            let index = request.state["index"].as_u64().unwrap_or_default();
            let active = self.active.fetch_add(1, Ordering::SeqCst) + 1;
            self.max_active.fetch_max(active, Ordering::SeqCst);
            sleep(Duration::from_millis(10)).await;
            self.active.fetch_sub(1, Ordering::SeqCst);
            if index == 1 {
                return Err(ClassificationError::Transient);
            }
            Ok(ClassificationResponse {
                answers: BTreeMap::new(),
                usage: ClassificationUsage {
                    input_tokens: Some(index),
                    output_tokens: None,
                },
                diagnostics: ClassificationDiagnostics::default(),
            })
        }
    }

    #[tokio::test]
    async fn batch_is_bounded_ordered_and_keeps_individual_failures() {
        let model = BatchModel {
            active: AtomicUsize::new(0),
            max_active: AtomicUsize::new(0),
        };
        let requests = (0..5)
            .map(|index| {
                let mut request = request();
                request.state = serde_json::json!({"index": index});
                request
            })
            .collect();
        let results = model
            .classify_batch(
                requests,
                ClassificationBatchOptions::new(NonZeroUsize::new(2).unwrap()),
            )
            .await;

        assert_eq!(results.len(), 5);
        assert_eq!(results[0].as_ref().unwrap().usage.input_tokens, Some(0));
        assert!(matches!(results[1], Err(ClassificationError::Transient)));
        assert_eq!(results[2].as_ref().unwrap().usage.input_tokens, Some(2));
        assert_eq!(results[4].as_ref().unwrap().usage.input_tokens, Some(4));
        assert_eq!(model.max_active.load(Ordering::SeqCst), 2);
    }

    struct CountModel(AtomicUsize);

    #[async_trait::async_trait]
    impl ClassifierModel for CountModel {
        async fn classify(
            &self,
            _request: ClassificationRequest,
        ) -> Result<ClassificationResponse, ClassificationError> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Ok(ClassificationResponse {
                answers: BTreeMap::new(),
                usage: ClassificationUsage::default(),
                diagnostics: ClassificationDiagnostics::default(),
            })
        }
    }

    #[tokio::test]
    async fn empty_batch_does_no_work() {
        let model = CountModel(AtomicUsize::new(0));
        let result = model
            .classify_batch(
                Vec::new(),
                ClassificationBatchOptions::new(NonZeroUsize::MIN),
            )
            .await;
        assert!(result.is_empty());
        assert_eq!(model.0.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn cancelled_batch_does_not_leave_detached_work() {
        struct SlowModel(Arc<AtomicUsize>);

        #[async_trait::async_trait]
        impl ClassifierModel for SlowModel {
            async fn classify(
                &self,
                _request: ClassificationRequest,
            ) -> Result<ClassificationResponse, ClassificationError> {
                sleep(Duration::from_millis(40)).await;
                self.0.fetch_add(1, Ordering::SeqCst);
                Ok(ClassificationResponse {
                    answers: BTreeMap::new(),
                    usage: ClassificationUsage::default(),
                    diagnostics: ClassificationDiagnostics::default(),
                })
            }
        }

        let completed = Arc::new(AtomicUsize::new(0));
        let model = Arc::new(SlowModel(Arc::clone(&completed)));
        let task = tokio::spawn(async move {
            let requests = (0..20).map(|_| request()).collect();
            model
                .classify_batch(
                    requests,
                    ClassificationBatchOptions::new(NonZeroUsize::new(2).unwrap()),
                )
                .await
        });
        sleep(Duration::from_millis(2)).await;
        task.abort();
        let _ = task.await;
        sleep(Duration::from_millis(60)).await;
        assert_eq!(completed.load(Ordering::SeqCst), 0);
    }

    struct IdentifiedBatchModel(AtomicUsize);

    #[async_trait::async_trait]
    impl ClassifierModel for IdentifiedBatchModel {
        fn provider_name(&self) -> &'static str {
            "test-provider"
        }

        fn model_name(&self) -> &'static str {
            "test-model"
        }

        async fn classify(
            &self,
            _request: ClassificationRequest,
        ) -> Result<ClassificationResponse, ClassificationError> {
            Err(ClassificationError::UnsupportedCapability)
        }

        async fn classify_batch(
            &self,
            requests: Vec<ClassificationRequest>,
            _options: ClassificationBatchOptions,
        ) -> Vec<Result<ClassificationResponse, ClassificationError>> {
            self.0.fetch_add(1, Ordering::SeqCst);
            requests
                .into_iter()
                .map(|_| Err(ClassificationError::UnsupportedCapability))
                .collect()
        }
    }

    #[tokio::test]
    async fn arc_forwarding_preserves_identity_and_batch_override() {
        let identified = Arc::new(IdentifiedBatchModel(AtomicUsize::new(0)));
        let concrete: Arc<dyn ClassifierModel> = identified.clone();
        assert_eq!(
            "test-provider",
            <Arc<dyn ClassifierModel> as ClassifierModel>::provider_name(&concrete)
        );
        assert_eq!(
            "test-model",
            <Arc<dyn ClassifierModel> as ClassifierModel>::model_name(&concrete)
        );

        let results = <Arc<dyn ClassifierModel> as ClassifierModel>::classify_batch(
            &concrete,
            vec![request()],
            ClassificationBatchOptions::new(NonZeroUsize::MIN),
        )
        .await;

        assert_eq!(1, results.len());
        assert!(matches!(
            results[0],
            Err(ClassificationError::UnsupportedCapability)
        ));
        assert_eq!(1, identified.0.load(Ordering::SeqCst));
    }

    #[test]
    fn probability_rejects_invalid_values_during_creation_and_deserialization() {
        for value in [-0.01, 1.01, f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            assert!(Probability::new(value).is_err());
        }
        for value in ["null", "\"0.5\"", "true", "-0.1", "1.1"] {
            assert!(serde_json::from_str::<Probability>(value).is_err());
        }
        assert_eq!(
            0.0,
            serde_json::from_str::<Probability>("0.0").unwrap().get()
        );
        assert_eq!(
            1.0,
            serde_json::from_str::<Probability>("1.0").unwrap().get()
        );
    }
}

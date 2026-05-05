use async_trait::async_trait;
use common::category_key::CategoryId;
use common::logging::{
    LlmInvocationMetrics, LlmModel, LlmOperation, LlmProvider, LogClassificationMethod,
    LogEventType, log_llm_invocation,
};
use common::period_key::PeriodId;
use llm::chat::ChatMessage;
use product::core::title::Title;
use product_classification::category::core::Category;
use product_classification::category::service::CategoryService;
use product_classification::period::core::Period;
use product_classification::period::service::PeriodService;
use std::time::Instant;
use thiserror::Error;
use tracing::{debug, info};

const CLEAR_SCORE_RATIO: f64 = 1.20;
const CANDIDATE_LIMIT_FOR_LLM: usize = 5;

#[derive(Debug, Error)]
pub enum ClassificationError {
    #[error("LLM request failed: {0}")]
    LlmError(#[from] llm::error::LLMError),
    #[error("OpenSearchError: {0}")]
    OpenSearchError(#[from] opensearch::Error),
    #[error("Invalid LLM response: {0}")]
    InvalidResponse(String),
    #[error("No {0} candidates found")]
    NoCandidates(&'static str),
}

#[async_trait]
#[mockall::automock]
pub trait ClassificationService {
    async fn classify(
        &self,
        title: &Title,
        embedding: &[f32],
    ) -> Result<(CategoryId, PeriodId), ClassificationError>;
}

pub struct ClassificationServiceImpl<'a> {
    llm: Box<dyn llm::LLMProvider>,
    category_service: &'a (dyn CategoryService + Send + Sync),
    period_service: &'a (dyn PeriodService + Send + Sync),
}

impl<'a> ClassificationServiceImpl<'a> {
    pub fn new(
        api_key: &str,
        category_service: &'a (dyn CategoryService + Send + Sync),
        period_service: &'a (dyn PeriodService + Send + Sync),
    ) -> Self {
        let llm = llm::builder::LLMBuilder::new()
            .backend(llm::builder::LLMBackend::Google)
            .api_key(api_key)
            .model("gemini-2.5-flash-lite")
            .system(
                "\
                You are an expert antiques classifier. \
                Given a product title and candidate categories and periods, \
                choose the single best matching category and period. \
                Respond ONLY with exactly two lines:\n\
                category: <chosen_category_id>\n\
                period: <chosen_period_id>",
            )
            .build()
            .expect("shouldn't fail building LLM provider");
        Self {
            llm,
            category_service,
            period_service,
        }
    }

    #[cfg(test)]
    pub fn new_with_provider(
        llm: Box<dyn llm::LLMProvider>,
        category_service: &'a (dyn CategoryService + Send + Sync),
        period_service: &'a (dyn PeriodService + Send + Sync),
    ) -> Self {
        Self {
            llm,
            category_service,
            period_service,
        }
    }
}

#[async_trait]
impl ClassificationService for ClassificationServiceImpl<'_> {
    async fn classify(
        &self,
        title: &Title,
        embedding: &[f32],
    ) -> Result<(CategoryId, PeriodId), ClassificationError> {
        let (categories, periods) = tokio::join!(
            self.category_service
                .find_category_candidates(title, embedding),
            self.period_service.find_period_candidates(title, embedding),
        );
        let categories = categories?;
        let periods = periods?;

        let category_ids = top_category_ids(&categories);
        let period_ids = top_period_ids(&periods);
        if category_ids.is_empty() {
            return Err(ClassificationError::NoCandidates("category"));
        }
        if period_ids.is_empty() {
            return Err(ClassificationError::NoCandidates("period"));
        }

        if let (Some(category_id), Some(period_id)) = (
            clear_category_candidate(&categories),
            clear_period_candidate(&periods),
        ) {
            info!(
                eventType = %LogEventType::ClassificationDecision,
                classificationMethod = %LogClassificationMethod::ClearScore,
                categoryId = %category_id,
                periodId = %period_id,
                categoryCandidateScores = format_candidates(&categories.iter().map(|(c, score)| (c.category_id.to_string(), *score)).collect::<Vec<_>>()),
                periodCandidateScores = format_candidates(&periods.iter().map(|(c, score)| (c.period_id.to_string(), *score)).collect::<Vec<_>>()),
                "Selected product classification from clear OpenSearch scores."
            );
            return Ok((category_id, period_id));
        }

        let categories_str = categories
            .iter()
            .take(CANDIDATE_LIMIT_FOR_LLM)
            .map(|(c, score)| format!("{} (score: {:.3})", c.category_id, score))
            .collect::<Vec<_>>()
            .join(", ");
        let periods_str = periods
            .iter()
            .take(CANDIDATE_LIMIT_FOR_LLM)
            .map(|(p, score)| format!("{} (score: {:.3})", p.period_id, score))
            .collect::<Vec<_>>()
            .join(", ");

        let user_message =
            format!("Product: {title}\nCategories: {categories_str}\nPeriods: {periods_str}");

        debug!("Requesting classification from Gemini API.");

        let started_at = Instant::now();
        let response = self
            .llm
            .chat(&[ChatMessage::user().content(&user_message).build()])
            .await?;
        log_llm_invocation(
            LlmOperation::ProductClassification,
            LlmProvider::Google,
            LlmModel::Gemini25FlashLite,
            started_at.elapsed(),
            llm_metrics(response.usage(), Some(1)),
        );

        let response_text = response.text().ok_or_else(|| {
            ClassificationError::InvalidResponse("Empty response from LLM".to_string())
        })?;

        let (category_id, period_id) =
            parse_classification_response(&response_text, &category_ids, &period_ids)?;
        info!(
            eventType = %LogEventType::ClassificationDecision,
            classificationMethod = %LogClassificationMethod::Llm,
            categoryId = %category_id,
            periodId = %period_id,
            categoryCandidateScores = format_candidates(&categories.iter().map(|(c, score)| (c.category_id.to_string(), *score)).collect::<Vec<_>>()),
            periodCandidateScores = format_candidates(&periods.iter().map(|(c, score)| (c.period_id.to_string(), *score)).collect::<Vec<_>>()),
            "Selected product classification with LLM."
        );
        Ok((category_id, period_id))
    }
}

fn llm_metrics(usage: Option<llm::chat::Usage>, batch_size: Option<usize>) -> LlmInvocationMetrics {
    let Some(usage) = usage else {
        return LlmInvocationMetrics {
            batch_size,
            ..Default::default()
        };
    };

    LlmInvocationMetrics {
        batch_size,
        prompt_tokens: Some(usage.prompt_tokens),
        completion_tokens: Some(usage.completion_tokens),
        total_tokens: Some(usage.total_tokens),
        cached_prompt_tokens: usage.prompt_tokens_details.and_then(|d| d.cached_tokens),
        reasoning_tokens: usage
            .completion_tokens_details
            .and_then(|d| d.reasoning_tokens),
        ..Default::default()
    }
}

fn format_candidates(candidates: &[(impl AsRef<str>, f64)]) -> String {
    let pairs = candidates
        .iter()
        .take(CANDIDATE_LIMIT_FOR_LLM)
        .map(|(candidate, score)| {
            (
                candidate.as_ref().to_owned(),
                serde_json::json!(format!("{score:.3}")),
            )
        })
        .collect::<serde_json::Map<String, serde_json::Value>>();
    serde_json::to_string(&serde_json::Value::Object(pairs))
        .unwrap_or("Failed serializing candidates".to_owned())
}

fn clear_category_candidate(categories: &[(Category, f64)]) -> Option<CategoryId> {
    clear_candidate(categories).map(|category| category.category_id.clone())
}

fn clear_period_candidate(periods: &[(Period, f64)]) -> Option<PeriodId> {
    clear_candidate(periods).map(|period| period.period_id.clone())
}

fn clear_candidate<T>(candidates: &[(T, f64)]) -> Option<&T> {
    let (first, first_score) = candidates.first()?;
    if candidates.len() == 1 {
        return Some(first);
    }
    let second_score = candidates.get(1).map(|(_, score)| *score).unwrap_or(0.0);
    if *first_score > 0.0 && *first_score >= second_score * CLEAR_SCORE_RATIO {
        Some(first)
    } else {
        None
    }
}

fn top_category_ids(categories: &[(Category, f64)]) -> Vec<CategoryId> {
    categories
        .iter()
        .take(CANDIDATE_LIMIT_FOR_LLM)
        .map(|(category, _)| category.category_id.clone())
        .collect()
}

fn top_period_ids(periods: &[(Period, f64)]) -> Vec<PeriodId> {
    periods
        .iter()
        .take(CANDIDATE_LIMIT_FOR_LLM)
        .map(|(period, _)| period.period_id.clone())
        .collect()
}

fn parse_classification_response(
    response: &str,
    categories: &[CategoryId],
    periods: &[PeriodId],
) -> Result<(CategoryId, PeriodId), ClassificationError> {
    let mut category_id = None;
    let mut period_id = None;

    for line in response.lines() {
        let line = line.trim();
        if let Some(cat) = line.strip_prefix("category:") {
            category_id = Some(CategoryId::from(cat.trim()));
        } else if let Some(per) = line.strip_prefix("period:") {
            period_id = Some(PeriodId::from(per.trim()));
        }
    }

    match (category_id, period_id) {
        (Some(cat), Some(per)) => {
            if !categories.contains(&cat) {
                return Err(ClassificationError::InvalidResponse(format!(
                    "Category '{cat}' is not in candidates"
                )));
            }
            if !periods.contains(&per) {
                return Err(ClassificationError::InvalidResponse(format!(
                    "Period '{per}' is not in candidates"
                )));
            }
            Ok((cat, per))
        }
        (None, _) => Err(ClassificationError::InvalidResponse(format!(
            "Could not parse category from response: {response}"
        ))),
        (_, None) => Err(ClassificationError::InvalidResponse(format!(
            "Could not parse period from response: {response}"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fake::{Fake, Faker};
    use llm::{LLMProvider, chat::ChatMessage, error::LLMError};
    use product_classification::category::service::MockCategoryService;
    use product_classification::period::service::MockPeriodService;
    use rstest::rstest;

    struct FakeChatResponse(Option<String>);

    impl std::fmt::Display for FakeChatResponse {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(f, "{}", self.0.as_deref().unwrap_or(""))
        }
    }

    impl std::fmt::Debug for FakeChatResponse {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(f, "FakeChatResponse({:?})", self.0)
        }
    }

    impl llm::chat::ChatResponse for FakeChatResponse {
        fn text(&self) -> Option<String> {
            self.0.clone()
        }

        fn tool_calls(&self) -> Option<Vec<llm::ToolCall>> {
            None
        }
    }

    struct PanicLlmProvider;

    #[async_trait::async_trait]
    impl llm::chat::ChatProvider for PanicLlmProvider {
        async fn chat_with_tools(
            &self,
            _messages: &[ChatMessage],
            _tools: Option<&[llm::chat::Tool]>,
        ) -> Result<Box<dyn llm::chat::ChatResponse>, LLMError> {
            panic!("LLM should not be called for unambiguous candidates")
        }
    }

    #[async_trait::async_trait]
    impl llm::completion::CompletionProvider for PanicLlmProvider {
        async fn complete(
            &self,
            _req: &llm::completion::CompletionRequest,
        ) -> Result<llm::completion::CompletionResponse, LLMError> {
            unimplemented!()
        }
    }

    #[async_trait::async_trait]
    impl llm::embedding::EmbeddingProvider for PanicLlmProvider {
        async fn embed(&self, _input: Vec<String>) -> Result<Vec<Vec<f32>>, LLMError> {
            unimplemented!()
        }
    }

    #[async_trait::async_trait]
    impl llm::stt::SpeechToTextProvider for PanicLlmProvider {
        async fn transcribe(&self, _audio: Vec<u8>) -> Result<String, LLMError> {
            unimplemented!()
        }
    }

    #[async_trait::async_trait]
    impl llm::tts::TextToSpeechProvider for PanicLlmProvider {}

    #[async_trait::async_trait]
    impl llm::models::ModelsProvider for PanicLlmProvider {}

    impl LLMProvider for PanicLlmProvider {}

    struct ReturningLlmProvider(String);

    #[async_trait::async_trait]
    impl llm::chat::ChatProvider for ReturningLlmProvider {
        async fn chat_with_tools(
            &self,
            _messages: &[ChatMessage],
            _tools: Option<&[llm::chat::Tool]>,
        ) -> Result<Box<dyn llm::chat::ChatResponse>, LLMError> {
            Ok(Box::new(FakeChatResponse(Some(self.0.clone()))))
        }
    }

    #[async_trait::async_trait]
    impl llm::completion::CompletionProvider for ReturningLlmProvider {
        async fn complete(
            &self,
            _req: &llm::completion::CompletionRequest,
        ) -> Result<llm::completion::CompletionResponse, LLMError> {
            unimplemented!()
        }
    }

    #[async_trait::async_trait]
    impl llm::embedding::EmbeddingProvider for ReturningLlmProvider {
        async fn embed(&self, _input: Vec<String>) -> Result<Vec<Vec<f32>>, LLMError> {
            unimplemented!()
        }
    }

    #[async_trait::async_trait]
    impl llm::stt::SpeechToTextProvider for ReturningLlmProvider {
        async fn transcribe(&self, _audio: Vec<u8>) -> Result<String, LLMError> {
            unimplemented!()
        }
    }

    #[async_trait::async_trait]
    impl llm::tts::TextToSpeechProvider for ReturningLlmProvider {}

    #[async_trait::async_trait]
    impl llm::models::ModelsProvider for ReturningLlmProvider {}

    impl LLMProvider for ReturningLlmProvider {}

    fn mk_categories() -> Vec<CategoryId> {
        vec![
            CategoryId::from("furniture"),
            CategoryId::from("decorative-objects"),
            CategoryId::from("paintings"),
        ]
    }

    fn mk_periods() -> Vec<PeriodId> {
        vec![
            PeriodId::from("baroque"),
            PeriodId::from("art-deco"),
            PeriodId::from("renaissance"),
        ]
    }

    #[tokio::test]
    async fn should_return_top_candidates_when_scores_are_clear_for_classification_service() {
        let mut top_category: Category = Faker.fake();
        top_category.category_id = CategoryId::from("furniture");
        let mut second_category: Category = Faker.fake();
        second_category.category_id = CategoryId::from("visual-art");
        let mut top_period: Period = Faker.fake();
        top_period.period_id = PeriodId::from("baroque");
        let mut second_period: Period = Faker.fake();
        second_period.period_id = PeriodId::from("rococo");

        let mut category_service = MockCategoryService::default();
        category_service
            .expect_find_category_candidates()
            .once()
            .return_once(move |_, _| {
                Box::pin(async move { Ok(vec![(top_category, 12.0), (second_category, 9.0)]) })
            });
        let mut period_service = MockPeriodService::default();
        period_service
            .expect_find_period_candidates()
            .once()
            .return_once(move |_, _| {
                Box::pin(async move { Ok(vec![(top_period, 7.0), (second_period, 5.0)]) })
            });

        let service = ClassificationServiceImpl::new_with_provider(
            Box::new(PanicLlmProvider),
            &category_service,
            &period_service,
        );

        let actual = service
            .classify(&Title::from("Baroque carved chair"), &[0.1, 0.2])
            .await
            .unwrap();

        assert_eq!(
            (CategoryId::from("furniture"), PeriodId::from("baroque")),
            actual
        );
    }

    #[tokio::test]
    async fn should_use_llm_when_candidate_scores_are_ambiguous_for_classification_service() {
        let mut category_a: Category = Faker.fake();
        category_a.category_id = CategoryId::from("furniture");
        let mut category_b: Category = Faker.fake();
        category_b.category_id = CategoryId::from("decorative-objects");
        let mut period_a: Period = Faker.fake();
        period_a.period_id = PeriodId::from("baroque");
        let mut period_b: Period = Faker.fake();
        period_b.period_id = PeriodId::from("rococo");

        let mut category_service = MockCategoryService::default();
        category_service
            .expect_find_category_candidates()
            .once()
            .return_once(move |_, _| {
                Box::pin(async move { Ok(vec![(category_a, 10.0), (category_b, 9.8)]) })
            });
        let mut period_service = MockPeriodService::default();
        period_service
            .expect_find_period_candidates()
            .once()
            .return_once(move |_, _| {
                Box::pin(async move { Ok(vec![(period_a, 8.0), (period_b, 7.9)]) })
            });

        let service = ClassificationServiceImpl::new_with_provider(
            Box::new(ReturningLlmProvider(
                "category: decorative-objects\nperiod: rococo".to_string(),
            )),
            &category_service,
            &period_service,
        );

        let actual = service
            .classify(&Title::from("Gilt rococo mirror"), &[0.1, 0.2])
            .await
            .unwrap();

        assert_eq!(
            (
                CategoryId::from("decorative-objects"),
                PeriodId::from("rococo")
            ),
            actual
        );
    }

    #[test]
    fn should_parse_valid_classification_response() {
        let response = "category: furniture\nperiod: baroque";
        let result = parse_classification_response(response, &mk_categories(), &mk_periods());
        let (cat, per) = result.unwrap();
        assert_eq!(cat, CategoryId::from("furniture"));
        assert_eq!(per, PeriodId::from("baroque"));
    }

    #[test]
    fn should_parse_response_with_extra_whitespace() {
        let response = "  category:  furniture  \n  period:  art-deco  ";
        let result = parse_classification_response(response, &mk_categories(), &mk_periods());
        let (cat, per) = result.unwrap();
        assert_eq!(cat, CategoryId::from("furniture"));
        assert_eq!(per, PeriodId::from("art-deco"));
    }

    #[test]
    fn should_parse_response_with_extra_lines() {
        let response =
            "Here is my classification:\ncategory: paintings\nperiod: renaissance\nThank you!";
        let result = parse_classification_response(response, &mk_categories(), &mk_periods());
        let (cat, per) = result.unwrap();
        assert_eq!(cat, CategoryId::from("paintings"));
        assert_eq!(per, PeriodId::from("renaissance"));
    }

    #[test]
    fn should_fail_when_category_missing_from_response() {
        let response = "period: baroque";
        let result = parse_classification_response(response, &mk_categories(), &mk_periods());
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("category"));
    }

    #[test]
    fn should_fail_when_period_missing_from_response() {
        let response = "category: furniture";
        let result = parse_classification_response(response, &mk_categories(), &mk_periods());
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("period"));
    }

    #[test]
    fn should_fail_when_category_not_in_candidates() {
        let response = "category: jewelry\nperiod: baroque";
        let result = parse_classification_response(response, &mk_categories(), &mk_periods());
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("not in candidates")
        );
    }

    #[test]
    fn should_fail_when_period_not_in_candidates() {
        let response = "category: furniture\nperiod: modern";
        let result = parse_classification_response(response, &mk_categories(), &mk_periods());
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("not in candidates")
        );
    }

    #[test]
    fn should_fail_when_response_is_empty() {
        let response = "";
        let result = parse_classification_response(response, &mk_categories(), &mk_periods());
        assert!(result.is_err());
    }

    #[test]
    fn should_fail_when_response_is_garbage() {
        let response = "I don't know what to classify this as.";
        let result = parse_classification_response(response, &mk_categories(), &mk_periods());
        assert!(result.is_err());
    }

    #[rstest]
    #[case("category: furniture\nperiod: baroque", "furniture", "baroque")]
    #[case(
        "category: decorative-objects\nperiod: art-deco",
        "decorative-objects",
        "art-deco"
    )]
    #[case("category: paintings\nperiod: renaissance", "paintings", "renaissance")]
    fn should_parse_all_valid_category_period_combinations(
        #[case] response: &str,
        #[case] expected_category: &str,
        #[case] expected_period: &str,
    ) {
        let result = parse_classification_response(response, &mk_categories(), &mk_periods());
        let (cat, per) = result.unwrap();
        assert_eq!(cat, CategoryId::from(expected_category));
        assert_eq!(per, PeriodId::from(expected_period));
    }
}

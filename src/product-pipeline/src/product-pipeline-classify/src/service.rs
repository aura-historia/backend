use async_trait::async_trait;
use common::category_key::CategoryId;
use common::period_key::PeriodId;
use llm::chat::ChatMessage;
use product::core::title::Title;
use thiserror::Error;
use tracing::debug;

#[derive(Debug, Error)]
pub enum ClassificationError {
    #[error("LLM request failed: {0}")]
    LlmError(#[from] llm::error::LLMError),
    #[error("Invalid LLM response: {0}")]
    InvalidResponse(String),
}

#[async_trait]
#[mockall::automock]
pub trait ClassificationService {
    async fn classify(
        &self,
        title: &Title,
        categories: &[CategoryId],
        periods: &[PeriodId],
    ) -> Result<(CategoryId, PeriodId), ClassificationError>;
}

pub struct ClassificationServiceImpl {
    llm: Box<dyn llm::LLMProvider>,
}

impl ClassificationServiceImpl {
    pub fn new(api_key: &str) -> Self {
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
        Self { llm }
    }
}

#[async_trait]
impl ClassificationService for ClassificationServiceImpl {
    async fn classify(
        &self,
        title: &Title,
        categories: &[CategoryId],
        periods: &[PeriodId],
    ) -> Result<(CategoryId, PeriodId), ClassificationError> {
        let categories_str = categories
            .iter()
            .map(|c| c.to_string())
            .collect::<Vec<_>>()
            .join(", ");
        let periods_str = periods
            .iter()
            .map(|p| p.to_string())
            .collect::<Vec<_>>()
            .join(", ");

        let user_message =
            format!("Product: {title}\nCategories: {categories_str}\nPeriods: {periods_str}");

        debug!("Requesting classification from Gemini API.");

        let response = self
            .llm
            .chat(&[ChatMessage::user().content(&user_message).build()])
            .await?;

        let response_text = response.text().ok_or_else(|| {
            ClassificationError::InvalidResponse("Empty response from LLM".to_string())
        })?;

        parse_classification_response(&response_text, categories, periods)
    }
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
    use rstest::rstest;

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

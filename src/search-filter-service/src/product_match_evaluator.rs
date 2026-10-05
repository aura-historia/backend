use classifier_model::{
    BinaryClassificationQuestion, ClassificationBatchOptions, ClassificationError,
    ClassificationOperation, ClassificationOptions, ClassificationRequest, ClassifierModel,
    Probability, QuestionId,
};
use localization::Language;
use product_listing_service::ports::ProductListingSearchFilterMatchSource;
use std::num::NonZeroUsize;

const HARD_CONFLICT_QUESTION: &str = "hard_conflict";
const SHOULD_SHOW_QUESTION: &str = "should_show";

pub(crate) struct ProductListingMatchEvaluationRequest<'a, Key> {
    pub(crate) key: Key,
    pub(crate) product: &'a ProductListingSearchFilterMatchSource,
    pub(crate) search_description: &'a str,
    pub(crate) search_language: Language,
}

pub(crate) struct ProductListingMatchEvaluationResult<Key> {
    pub(crate) key: Key,
    pub(crate) outcome: ProductListingMatchEvaluationOutcome,
}

pub(crate) enum ProductListingMatchEvaluationOutcome {
    Matched,
    Rejected,
    RetryableFailure(ClassificationError),
    PermanentFailure(ClassificationError),
}

pub(crate) async fn evaluate_product_matches<E, Key>(
    classifier: &E,
    evaluations: Vec<ProductListingMatchEvaluationRequest<'_, Key>>,
    max_concurrent_requests: NonZeroUsize,
    should_show_threshold: Probability,
) -> Vec<ProductListingMatchEvaluationResult<Key>>
where
    E: ClassifierModel,
{
    let (keys, requests): (Vec<_>, Vec<_>) = evaluations
        .into_iter()
        .map(|evaluation| {
            (
                evaluation.key,
                product_match_request(
                    evaluation.product,
                    evaluation.search_description,
                    evaluation.search_language,
                ),
            )
        })
        .unzip();
    if keys.is_empty() {
        return Vec::new();
    }
    let results = classifier
        .classify_batch(
            requests,
            ClassificationBatchOptions::new(max_concurrent_requests),
        )
        .await;

    if results.len() != keys.len() {
        return keys
            .into_iter()
            .map(|key| ProductListingMatchEvaluationResult {
                key,
                outcome: ProductListingMatchEvaluationOutcome::RetryableFailure(
                    ClassificationError::InvalidResponse,
                ),
            })
            .collect();
    }

    keys.into_iter()
        .zip(results)
        .map(|(key, result)| ProductListingMatchEvaluationResult {
            key,
            outcome: match result.and_then(|response| {
                response
                    .answers
                    .get(&question_id(SHOULD_SHOW_QUESTION))
                    .copied()
                    .ok_or(ClassificationError::InvalidResponse)
            }) {
                Ok(should_show) if should_show.get() >= should_show_threshold.get() => {
                    ProductListingMatchEvaluationOutcome::Matched
                }
                Ok(_) => ProductListingMatchEvaluationOutcome::Rejected,
                Err(ClassificationError::PermanentCandidateFailure) => {
                    ProductListingMatchEvaluationOutcome::PermanentFailure(
                        ClassificationError::PermanentCandidateFailure,
                    )
                }
                Err(error) => ProductListingMatchEvaluationOutcome::RetryableFailure(error),
            },
        })
        .collect()
}

fn product_match_request(
    product: &ProductListingSearchFilterMatchSource,
    search_description: &str,
    search_language: Language,
) -> ClassificationRequest {
    let (title, description) = product_text(product, search_language);
    ClassificationRequest {
        operation: ClassificationOperation::ProductEnhancedSearchDescriptionMatching,
        state: serde_json::json!({
            "user_search": search_description,
            "candidate": {
                "title": title,
                "description": description,
            },
            "evaluation_context": {
                "purpose": "saved_search_matching",
                "search_language": search_language.format_human_readable(),
                "hard_conflict_is_diagnostic_only": true,
            },
        }),
        questions: vec![
            BinaryClassificationQuestion {
                id: question_id(HARD_CONFLICT_QUESTION),
                instructions: "What is the probability that the listing clearly contradicts a non-negotiable requirement stated by the user? Missing evidence alone is not a contradiction.".to_owned(),
            },
            BinaryClassificationQuestion {
                id: question_id(SHOULD_SHOW_QUESTION),
                instructions: "Considering the user's entire description, including hard requirements, preferences, ranges, exclusions, and uncertainty, what is the probability this listing should be shown as a saved-search match? A preference mismatch alone does not necessarily mean it should be hidden.".to_owned(),
            },
        ],
        image_urls: product
            .images
            .iter()
            .take(1)
            .map(|image| image.url().clone())
            .collect(),
        options: ClassificationOptions::default(),
    }
}

fn question_id(value: &str) -> QuestionId {
    QuestionId::new(value).unwrap_or_else(|_| unreachable!("static classification question ID"))
}

fn product_text(
    product: &ProductListingSearchFilterMatchSource,
    search_language: Language,
) -> (&str, &str) {
    let title = product
        .titles
        .get(&search_language)
        .or_else(|| product.titles.get(&Language::En))
        .map(AsRef::as_ref)
        .or_else(|| {
            product
                .product_title
                .as_ref()
                .map(|title| title.payload.as_ref())
        })
        .unwrap_or("");
    let description = product
        .descriptions
        .get(&search_language)
        .or_else(|| product.descriptions.get(&Language::En))
        .map(AsRef::as_ref)
        .unwrap_or("");
    (title, description)
}

#[cfg(test)]
mod tests {
    use super::*;
    use domain_primitives::event_id::EventId;
    use indexmap::IndexSet;
    use listing_source_core::{ListingSourceId, ListingSourceName, ListingSourceSlugId};
    use product_listing_core::{
        listing_availability::ListingAvailability, listing_lifecycle::ListingLifecycle,
        product_listing::ProductListingPricing, product_listing_image::ProductListingImage,
        product_listing_slug_id::ProductListingSlugId, source_listing_id::SourceListingId,
    };
    use product_listing_service::ports::{
        ListingSourceSummary, ProductListingSearchFilterMatchSourceEventKind,
    };
    use std::{collections::BTreeMap, sync::Arc};
    use url::Url;

    fn product() -> Result<ProductListingSearchFilterMatchSource, url::ParseError> {
        let url = Url::parse("https://example.test/product")?;
        let event_id = EventId::new();
        Ok(ProductListingSearchFilterMatchSource {
            event_id,
            event_kind: ProductListingSearchFilterMatchSourceEventKind::Domain,
            origin_event_time: time::OffsetDateTime::UNIX_EPOCH,
            current_event_id: event_id,
            projection_version: 1,
            product_listing_id: product_listing_core::product_listing_id::ProductListingId::new(),
            product_listing_title_slug_id: ProductListingSlugId::raw("product-a1b2c3")
                .unwrap_or_else(|error| panic!("valid product listing title slug: {error}")),
            source: ListingSourceSummary {
                listing_source_id: ListingSourceId::new(),
                name: ListingSourceName::try_from("Source")
                    .unwrap_or_else(|error| panic!("invalid test listing source name: {error}")),
                slug_id: ListingSourceSlugId::raw("source")
                    .unwrap_or_else(|error| panic!("valid test listing source slug: {error}")),
            },
            source_listing_id: SourceListingId::try_from("product")
                .unwrap_or_else(|error| panic!("valid source listing ID: {error}")),
            product_title: None,
            product_description: None,
            titles: std::collections::HashMap::new(),
            descriptions: std::collections::HashMap::new(),
            pricing: ProductListingPricing::default(),
            sale_observation: None,
            availability: Some(ListingAvailability::Available),
            lifecycle: ListingLifecycle::Active,
            url: url.clone(),
            view_url: url,
            image: None,
            images: IndexSet::new(),
            embedding: None,
            auction: None,
            created: time::OffsetDateTime::UNIX_EPOCH,
            updated: time::OffsetDateTime::UNIX_EPOCH,
        })
    }

    #[test]
    fn request_uses_raw_search_localized_text_and_only_first_image() -> Result<(), url::ParseError>
    {
        let mut product = product()?;
        product.titles.insert(Language::En, "Brass lamp".into());
        product
            .descriptions
            .insert(Language::En, "From 1920".into());
        let image_urls = (0..3)
            .map(|index| Url::parse(&format!("https://example.test/image-{index}.jpg")))
            .collect::<Result<Vec<_>, _>>()?;
        for url in &image_urls {
            product.images.insert(ProductListingImage::new(url.clone()));
        }

        let request = product_match_request(&product, "only antique brass", Language::En);
        assert_eq!(request.state["user_search"], "only antique brass");
        assert_eq!(request.state["candidate"]["title"], "Brass lamp");
        assert_eq!(request.state["candidate"]["description"], "From 1920");
        assert_eq!(request.questions.len(), 2);
        assert_eq!(request.questions[0].id.as_str(), HARD_CONFLICT_QUESTION);
        assert_eq!(request.questions[1].id.as_str(), SHOULD_SHOW_QUESTION);
        assert_eq!(request.image_urls, vec![image_urls[0].clone()]);
        Ok(())
    }

    struct OrderedClassifier;

    #[async_trait::async_trait]
    impl ClassifierModel for OrderedClassifier {
        async fn classify(
            &self,
            request: ClassificationRequest,
        ) -> Result<classifier_model::ClassificationResponse, ClassificationError> {
            let search = request.state["user_search"].as_str().unwrap_or_default();
            let should_show = if search == "inclusive boundary" {
                0.5
            } else {
                0.49
            };
            Ok(classifier_model::ClassificationResponse {
                answers: BTreeMap::from([(
                    question_id(SHOULD_SHOW_QUESTION),
                    Probability::new(should_show)?,
                )]),
                usage: classifier_model::ClassificationUsage::default(),
            })
        }
    }

    #[tokio::test]
    async fn should_show_threshold_is_inclusive_and_batch_order_is_preserved()
    -> Result<(), Box<dyn std::error::Error>> {
        let product = product()?;
        let results = evaluate_product_matches(
            &OrderedClassifier,
            vec![
                ProductListingMatchEvaluationRequest {
                    key: "first",
                    product: &product,
                    search_description: "inclusive boundary",
                    search_language: Language::En,
                },
                ProductListingMatchEvaluationRequest {
                    key: "second",
                    product: &product,
                    search_description: "below boundary",
                    search_language: Language::En,
                },
            ],
            NonZeroUsize::MIN,
            Probability::new(0.50)?,
        )
        .await;

        assert_eq!(results.len(), 2);
        assert_eq!(results[0].key, "first");
        assert!(matches!(
            results[0].outcome,
            ProductListingMatchEvaluationOutcome::Matched
        ));
        assert_eq!(results[1].key, "second");
        assert!(matches!(
            results[1].outcome,
            ProductListingMatchEvaluationOutcome::Rejected
        ));
        Ok(())
    }

    struct MalformedBatchClassifier;

    #[async_trait::async_trait]
    impl ClassifierModel for MalformedBatchClassifier {
        async fn classify(
            &self,
            _request: ClassificationRequest,
        ) -> Result<classifier_model::ClassificationResponse, ClassificationError> {
            Err(ClassificationError::Transient)
        }

        async fn classify_batch(
            &self,
            _requests: Vec<ClassificationRequest>,
            _options: ClassificationBatchOptions,
        ) -> Vec<Result<classifier_model::ClassificationResponse, ClassificationError>> {
            Vec::new()
        }
    }

    #[tokio::test]
    async fn malformed_batch_cardinality_fails_every_affected_candidate()
    -> Result<(), Box<dyn std::error::Error>> {
        let product = product()?;
        let results = evaluate_product_matches(
            &MalformedBatchClassifier,
            vec![ProductListingMatchEvaluationRequest {
                key: "candidate",
                product: &product,
                search_description: "table",
                search_language: Language::En,
            }],
            NonZeroUsize::MIN,
            Probability::new(0.5)?,
        )
        .await;
        assert_eq!(results.len(), 1);
        assert!(matches!(
            results[0].outcome,
            ProductListingMatchEvaluationOutcome::RetryableFailure(
                ClassificationError::InvalidResponse
            )
        ));
        Ok(())
    }

    struct ArcBatchOverrideClassifier;

    #[async_trait::async_trait]
    impl ClassifierModel for ArcBatchOverrideClassifier {
        async fn classify(
            &self,
            _request: ClassificationRequest,
        ) -> Result<classifier_model::ClassificationResponse, ClassificationError> {
            Err(ClassificationError::Transient)
        }

        async fn classify_batch(
            &self,
            requests: Vec<ClassificationRequest>,
            _options: ClassificationBatchOptions,
        ) -> Vec<Result<classifier_model::ClassificationResponse, ClassificationError>> {
            vec![
                Ok(classifier_model::ClassificationResponse {
                    answers: BTreeMap::from([(
                        question_id(SHOULD_SHOW_QUESTION),
                        Probability::new(0.75).unwrap(),
                    )]),
                    usage: classifier_model::ClassificationUsage::default(),
                });
                requests.len()
            ]
        }
    }

    #[tokio::test]
    async fn arc_forwarding_preserves_batch_overrides() -> Result<(), Box<dyn std::error::Error>> {
        let product = product()?;
        let classifier: Arc<dyn ClassifierModel> = Arc::new(ArcBatchOverrideClassifier);
        let results = evaluate_product_matches(
            &classifier,
            vec![ProductListingMatchEvaluationRequest {
                key: "candidate",
                product: &product,
                search_description: "table",
                search_language: Language::En,
            }],
            NonZeroUsize::MIN,
            Probability::new(0.5)?,
        )
        .await;
        assert!(matches!(
            results[0].outcome,
            ProductListingMatchEvaluationOutcome::Matched
        ));
        Ok(())
    }
}

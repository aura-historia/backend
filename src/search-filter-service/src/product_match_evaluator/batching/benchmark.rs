//! Manual, opt-in provider benchmark. Ordinary library validation never calls a live model.
use super::{BATCH_PROMPT_VERSION, classify_matching_requests, plan_requests};
use crate::product_match_evaluator::{match_scores, product_match_request};
use classifier_model::{
    ClassificationError, ClassificationRequest, ClassificationRequestLimits,
    ClassificationResponse, ClassifierModel, CloudflareClassifierConfig, CloudflareClassifierModel,
    CloudflareModel,
};
use localization::Language;
use product_listing_core::product_listing_search::ProductListingSearch;
use search_filter_core::MatchingEvidencePolicy;
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    num::NonZeroUsize,
    sync::Mutex,
    time::Instant,
};

struct Observation {
    millis: u64,
    shared: bool,
    candidates: usize,
    input_tokens: Option<u64>,
    output_tokens: Option<u64>,
    unresolved_questions: usize,
    invalid_envelope: bool,
    failed: bool,
}

struct MeasuredModel<M> {
    model: M,
    observations: Mutex<Vec<Observation>>,
}

#[async_trait::async_trait]
impl<M: ClassifierModel> ClassifierModel for MeasuredModel<M> {
    fn request_limits(&self) -> Option<ClassificationRequestLimits> {
        self.model.request_limits()
    }
    async fn classify(
        &self,
        request: ClassificationRequest,
    ) -> Result<ClassificationResponse, ClassificationError> {
        let questions = request.questions.len();
        let shared = request.state.get("candidates").is_some();
        let candidates = request.state["candidates"].as_array().map_or(1, Vec::len);
        let start = Instant::now();
        let result = self.model.classify(request).await;
        self.observations.lock().unwrap().push(Observation {
            millis: u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX),
            shared,
            candidates,
            input_tokens: result.as_ref().ok().and_then(|r| r.usage.input_tokens),
            output_tokens: result.as_ref().ok().and_then(|r| r.usage.output_tokens),
            unresolved_questions: result
                .as_ref()
                .map(|r| questions.saturating_sub(r.answers.len()))
                .unwrap_or(questions),
            invalid_envelope: matches!(result, Err(ClassificationError::InvalidResponse)),
            failed: result.is_err(),
        });
        result
    }
}

struct Fixture {
    id: String,
    label: bool,
    request: ClassificationRequest,
}

struct Scored {
    score: Option<f64>,
    label: bool,
    fallback: bool,
    requested_image_count: usize,
    sent_image_count: Option<usize>,
}

fn required_text<'a>(value: &'a Value, name: &str) -> &'a str {
    value[name]
        .as_str()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| panic!("missing fixture field: {name}"))
}

fn nullable_text<'a>(value: &'a Value, name: &str) -> Option<&'a str> {
    match &value[name] {
        Value::Null => None,
        Value::String(text) => Some(text),
        _ => panic!("fixture field must be text or null: {name}"),
    }
}

fn fixture_product(
    case: &Value,
    language: Language,
) -> product_listing_service::ports::ProductListingSearchFilterMatchSource {
    let mut product = crate::product_match_evaluator::tests::product().unwrap();
    if let Some(authored) = nullable_text(case, "title") {
        let title: product_listing_core::title::Title = authored.into();
        assert!(
            authored == title.as_ref(),
            "fixture title would be normalized or truncated"
        );
        product.titles.insert(language, title);
    }
    if let Some(authored) = nullable_text(case, "description") {
        let description: product_listing_core::description::Description = authored.into();
        assert!(
            authored == description.as_ref(),
            "fixture description would be normalized or truncated"
        );
        product.descriptions.insert(language, description);
    }
    if let Some(images) = case.get("image_urls") {
        let images = images.as_array().expect("fixture image URLs");
        assert!(images.len() <= 4, "at most four reviewed images");
        for image in images {
            let url = url::Url::parse(image.as_str().expect("image URL string"))
                .expect("valid image URL");
            assert!(
                product.images.insert(
                    product_listing_core::product_listing_image::ProductListingImage::new(url)
                ),
                "unique reviewed images"
            );
        }
    }
    product
}

fn fixtures(manifest: &Value) -> Vec<Vec<Fixture>> {
    // The initial schema comparison can use the existing hard cases. The larger
    // multilingual coverage gate applies before evaluating shared requests.
    let batching = manifest["batch_candidates"].as_u64() != Some(1);
    let human_reviewed = human_reviewed(manifest);
    required_text(manifest, "dataset_revision");
    required_text(
        manifest,
        if human_reviewed {
            "reviewed_by"
        } else {
            "authored_by"
        },
    );
    let mut ids = BTreeSet::new();
    let mut languages = BTreeSet::new();
    let mut tags = BTreeSet::new();
    let mut positives = 0;
    let mut negatives = 0;
    let mut groups = Vec::new();
    for group in manifest["groups"].as_array().expect("fixture groups") {
        let code = required_text(group, "language");
        let language = Language::from_code(code).expect("canonical ISO language code");
        languages.insert(code);
        let mut search = ProductListingSearch::new(language, money::Currency::Eur);
        for query in group["base_queries"].as_array().expect("base queries") {
            let authored = query.as_str().expect("query string");
            let query: domain_primitives::query::text_query::TextQuery<1> =
                authored.try_into().expect("valid base query");
            assert!(
                authored == query.as_ref(),
                "fixture base query would be truncated"
            );
            search = search.with_product_listing_query(query);
        }
        if let Some(enhanced) = nullable_text(group, "enhanced_description") {
            let description: product_listing_core::product_listing_search::EnhancedSearchDescription =
                enhanced.try_into().expect("valid enhanced instructions");
            // Benchmark comparisons require the complete authored fixture intent.
            assert!(
                enhanced == description.as_ref(),
                "fixture enhanced instructions would be normalized or truncated"
            );
            search = search.with_enhanced_search_description(description);
        }
        assert!(
            !search.product_listing_query.is_empty()
                || search.enhanced_search_description.is_some()
        );
        let policy = MatchingEvidencePolicy::from_code(required_text(group, "evidence_policy"))
            .expect("resolved evidence policy");
        assert!(search.enhanced_search_description.is_none() || policy.permits_enhanced_intent());
        let cases = group["cases"].as_array().expect("labelled cases");
        assert!(
            ((if batching { 2 } else { 1 })..=50).contains(&cases.len()),
            "each group is one frozen search selection"
        );
        let mut group_positive = false;
        let mut group_negative = false;
        let mut group_fixtures = Vec::new();
        for case in cases {
            let id = required_text(case, "id");
            classifier_model::QuestionId::new(id).expect("opaque case ID");
            assert!(
                id.len() <= 100 && ids.insert(id.to_owned()),
                "unique bounded case IDs"
            );
            let label = case[if human_reviewed {
                "human_match"
            } else {
                "expected_match"
            }]
            .as_bool()
            .expect("expected match decision");
            if label {
                positives += 1;
                group_positive = true;
            } else {
                negatives += 1;
                group_negative = true;
            }
            for tag in case["tags"].as_array().expect("coverage tags") {
                tags.insert(tag.as_str().expect("tag string").to_owned());
            }
            let product = fixture_product(case, language);
            assert!(
                product.images.len() <= policy.image_limit(),
                "reviewed images must fit the evidence policy"
            );
            let mut request = product_match_request(&product, &search, policy);
            request.correlation_id = Some(format!("benchmark:{id}"));
            group_fixtures.push(Fixture {
                id: id.into(),
                label,
                request,
            });
        }
        assert!(
            !batching || (group_positive && group_negative),
            "neighbor groups must mix positive and negative evidence"
        );
        groups.push(group_fixtures);
    }
    assert!(
        positives > 0 && negatives > 0,
        "positive and negative labels required"
    );
    if !batching {
        return groups;
    }
    assert!(
        languages.len() >= 3 && positives >= 20 && negatives >= 20,
        "representative multilingual positives and negatives required"
    );
    for tag in [
        "easy_positive",
        "hard_positive",
        "easy_negative",
        "hard_negative",
        "missing_description",
        "missing_images",
        "long_instructions",
        "contradictory_neighbors",
        "injection",
        "antiques",
        "art",
    ] {
        assert!(tags.contains(tag), "missing coverage tag: {tag}");
    }
    groups
}

fn human_reviewed(manifest: &Value) -> bool {
    match manifest["label_provenance"].as_str() {
        Some("human-reviewed") => true,
        Some("agent-authored") => false,
        _ => panic!("unsupported benchmark label provenance"),
    }
}

fn rollout_gate(manifest: &Value, batch_size: usize) -> &'static str {
    if !human_reviewed(manifest) {
        "INELIGIBLE_AGENT_AUTHORED_LABELS"
    } else if batch_size == 1 {
        "PENDING_HUMAN_SCHEMA_REVIEW"
    } else {
        "PENDING_HUMAN_REVIEW_AND_RECONCILED_COSTS"
    }
}

// This schema is confined to the manual comparison; it is never live matching input.
fn baseline_request(mut request: ClassificationRequest) -> ClassificationRequest {
    request.state["evaluation_context"]["hard_conflict_is_diagnostic_only"] = true.into();
    request.questions.insert(0, classifier_model::BinaryClassificationQuestion {
        id: super::super::question_id("hard_conflict"),
        instructions: "Is there clear evidence that this listing contradicts a non-negotiable requirement stated by the user? Missing evidence alone is not a contradiction.".to_owned(),
    });
    request
}

fn measured_score(response: &ClassificationResponse, baseline: bool) -> Option<f64> {
    let mut response = response.clone();
    if baseline
        && response
            .answers
            .remove(&super::super::question_id("hard_conflict"))
            .is_none()
    {
        return None;
    }
    match_scores(&response)
        .ok()
        .map(classifier_model::Probability::get)
}

async fn measure<M: ClassifierModel>(
    model: &MeasuredModel<M>,
    groups: &[Vec<Fixture>],
    batch_size: usize,
    reverse: bool,
    threshold: f64,
    token_price: f64,
) -> (Value, BTreeMap<String, Scored>) {
    measure_schema(
        model,
        groups,
        batch_size,
        reverse,
        threshold,
        token_price,
        false,
    )
    .await
}

async fn measure_schema<M: ClassifierModel>(
    model: &MeasuredModel<M>,
    groups: &[Vec<Fixture>],
    batch_size: usize,
    reverse: bool,
    threshold: f64,
    token_price: f64,
    baseline: bool,
) -> (Value, BTreeMap<String, Scored>) {
    assert!(
        !baseline || batch_size == 1,
        "baseline comparison uses singleton requests"
    );
    model.observations.lock().unwrap().clear();
    let started = Instant::now();
    let mut scores = BTreeMap::new();
    let mut planned_count = 0;
    let mut initial_singletons = BTreeSet::new();
    for group in groups {
        let mut order = group.iter().collect::<Vec<_>>();
        if reverse {
            order.reverse();
        }
        let requests = order
            .iter()
            .map(|f| {
                if baseline {
                    baseline_request(f.request.clone())
                } else {
                    f.request.clone()
                }
            })
            .collect::<Vec<_>>();
        let size = NonZeroUsize::new(batch_size).unwrap();
        let plan = plan_requests(&requests, model.request_limits(), size);
        planned_count += plan.len();
        for item in &plan {
            if item.candidates.len() == 1 {
                initial_singletons.insert(order[item.candidates[0]].id.clone());
            }
        }
        let results =
            classify_matching_requests(model, requests, NonZeroUsize::new(8).unwrap(), size).await;
        for (fixture, result) in order.into_iter().zip(results) {
            scores.insert(
                fixture.id.clone(),
                Scored {
                    score: result
                        .result
                        .as_ref()
                        .ok()
                        // A/B quality comparisons require the complete reviewed evidence.
                        .filter(|response| {
                            response.diagnostics.sent_image_count
                                == fixture.request.image_urls.len()
                        })
                        .and_then(|response| measured_score(response, baseline)),
                    label: fixture.label,
                    requested_image_count: fixture.request.image_urls.len(),
                    sent_image_count: result
                        .result
                        .as_ref()
                        .ok()
                        .map(|response| response.diagnostics.sent_image_count),
                    fallback: result.prompt_version == super::super::MATCH_PROMPT_VERSION
                        && !initial_singletons.contains(&fixture.id),
                },
            );
        }
    }
    let mut tp = 0;
    let mut fp = 0;
    let mut tn = 0;
    let mut fn_count = 0;
    let mut unresolved = 0;
    let mut unresolved_positive = 0;
    for fixture in groups.iter().flatten() {
        match scores[&fixture.id].score {
            Some(show) => match (fixture.label, show >= threshold) {
                (true, true) => tp += 1,
                (false, true) => fp += 1,
                (false, false) => tn += 1,
                (true, false) => fn_count += 1,
            },
            None => {
                unresolved += 1;
                if fixture.label {
                    unresolved_positive += 1;
                }
            }
        }
    }
    let observations = model.observations.lock().unwrap();
    let mut durations = observations.iter().map(|o| o.millis).collect::<Vec<_>>();
    durations.sort_unstable();
    let known_tokens = observations
        .iter()
        .filter_map(|o| o.input_tokens)
        .sum::<u64>();
    let unknown_token_requests = observations
        .iter()
        .filter(|o| o.input_tokens.is_none())
        .count();
    let precision = ratio(tp, tp + fp);
    let recall = ratio(tp, tp + fn_count + unresolved_positive);
    let report = json!({
        "candidates": scores.len(), "true_positive": tp, "false_positive": fp, "true_negative": tn,
        "false_negative": fn_count, "unresolved_candidates": unresolved,
        "precision": precision, "recall_including_unresolved": recall,
        "elapsed_millis": started.elapsed().as_millis(),
        "request_p50_millis": durations[durations.len() / 2],
        "request_p95_millis": durations[(durations.len() * 95 / 100).min(durations.len() - 1)],
        "provider_requests": observations.len(), "planned_requests": planned_count,
        "shared_provider_requests": observations.iter().filter(|o| o.shared).count(),
        "largest_request_candidates": observations.iter().map(|o| o.candidates).max(),
        "request_candidate_counts": observations.iter().fold(BTreeMap::<usize, usize>::new(), |mut counts, o| { *counts.entry(o.candidates).or_default() += 1; counts }),
        "fallback_requests": observations.len().saturating_sub(planned_count),
        "fallback_candidates": scores.values().filter(|s| s.fallback).count(),
        "failed_requests": observations.iter().filter(|o| o.failed).count(),
        "unresolved_questions": observations.iter().map(|o| o.unresolved_questions).sum::<usize>(),
        "invalid_response_envelopes": observations.iter().filter(|o| o.invalid_envelope).count(),
        "known_input_tokens": known_tokens, "unknown_token_requests": unknown_token_requests,
        "known_output_tokens": observations.iter().filter_map(|o| o.output_tokens).sum::<u64>(),
        "unknown_output_token_requests": observations.iter().filter(|o| o.output_tokens.is_none()).count(),
        "token_priced_cost_usd": if unknown_token_requests == 0 { Some(known_tokens as f64 * token_price / 1_000_000.0) } else { None },
        "valid_response_rate": ratio(observations.iter().filter(|o| !o.failed && o.unresolved_questions == 0).count(), observations.len()),
        "incomplete_response_rate": ratio(observations.iter().filter(|o| !o.failed && o.unresolved_questions > 0).count(), observations.len()),
        "invalid_response_rate": ratio(observations.iter().filter(|o| o.invalid_envelope).count(), observations.len()),
        "incomplete_evidence_candidates": scores.values().filter(|s| s.sent_image_count.is_some_and(|count| count != s.requested_image_count)).count(),
        "scores": scores.iter().map(|(id, s)| json!({"id": id, "expected_match": s.label, "should_show": s.score, "decision": s.score.map(|score| score >= threshold), "requested_image_count": s.requested_image_count, "sent_image_count": s.sent_image_count})).collect::<Vec<_>>()
    });
    (report, scores)
}

fn ratio(numerator: usize, denominator: usize) -> Option<f64> {
    (denominator > 0).then(|| numerator as f64 / denominator as f64)
}

fn compare(
    left: &BTreeMap<String, Scored>,
    right: &BTreeMap<String, Scored>,
    threshold: f64,
) -> Value {
    let mut compared = 0;
    let mut disagreement = 0;
    let mut show_delta = 0.0;
    let mut flips = Vec::new();
    let mut evidence_disagreement = Vec::new();
    let mut max_show_delta: f64 = 0.0;
    for (id, left) in left {
        let right = &right[id];
        assert_eq!(left.label, right.label, "identical expected labels");
        if left.sent_image_count != right.sent_image_count {
            evidence_disagreement.push(id);
        }
        if let (Some(left_show), Some(right_show)) = (left.score, right.score) {
            compared += 1;
            disagreement += usize::from((left_show >= threshold) != (right_show >= threshold));
            let delta = (left_show - right_show).abs();
            show_delta += delta;
            if (left_show >= threshold) != (right_show >= threshold) {
                flips.push(
                    json!({"id": id, "expected_match": left.label, "left_should_show": left_show,
                    "right_should_show": right_show, "left_decision": left_show >= threshold,
                    "right_decision": right_show >= threshold,
                    "left_correct_right_wrong": (left_show >= threshold) == left.label}),
                );
            }
            max_show_delta = max_show_delta.max(delta);
        }
    }
    json!({"compared_candidates": compared, "decision_disagreement": ratio(disagreement, compared),
        "should_show_mean_absolute_delta": (compared > 0).then(|| show_delta / compared as f64),
        "decision_flips": flips, "image_evidence_disagreement": evidence_disagreement,
        "should_show_max_absolute_delta": max_show_delta})
}

/// Requires explicit label provenance and a dedicated test account. Writes only aggregate
/// metrics and opaque case IDs/scores; never fixture text or credentials.
#[tokio::test]
#[ignore = "manual labelled provider benchmark; requires explicit test-account opt-in"]
async fn saved_search_batch_quality_benchmark() {
    assert_eq!(
        std::env::var("AURA_BENCH_ALLOW_PROVIDER").as_deref(),
        Ok("1")
    );
    let fixture_path = std::env::var("AURA_BENCH_FIXTURES").expect("fixture path");
    let manifest: Value =
        serde_json::from_slice(&std::fs::read(fixture_path).expect("fixture file"))
            .expect("fixture JSON");
    let groups = fixtures(&manifest);
    let threshold = manifest["should_show_threshold"]
        .as_f64()
        .filter(|v| *v == 0.5)
        .expect("unchanged inclusive 0.5 comparison threshold");
    let batch_size = manifest["batch_candidates"]
        .as_u64()
        .filter(|v| (1..=50).contains(v))
        .expect("batch size") as usize;
    let token_price = manifest["input_usd_per_million_tokens"]
        .as_f64()
        .filter(|v| v.is_finite() && *v > 0.0)
        .expect("verified current token price");
    let configured_model: CloudflareModel = nullable_text(&manifest, "classifier_model")
        .unwrap_or("clef-flash")
        .parse()
        .expect("supported configured benchmark model");
    let model = MeasuredModel {
        model: CloudflareClassifierModel::new(
            CloudflareClassifierConfig::new(
                std::env::var("AURA_BENCH_CLOUDFLARE_ACCOUNT_ID")
                    .expect("dedicated benchmark account"),
                std::env::var("AURA_BENCH_CLOUDFLARE_API_TOKEN")
                    .expect("dedicated benchmark token"),
                configured_model,
            )
            .expect("benchmark provider config"),
        )
        .expect("benchmark provider"),
        observations: Mutex::default(),
    };
    assert!(
        batch_size == 1
            || groups.iter().any(|group| {
                let requests = group
                    .iter()
                    .map(|fixture| fixture.request.clone())
                    .collect::<Vec<_>>();
                plan_requests(
                    &requests,
                    model.request_limits(),
                    NonZeroUsize::new(batch_size).unwrap(),
                )
                .iter()
                .any(|plan| plan.candidates.len() > 1)
            }),
        "benchmark fixtures must exercise shared requests within provider budgets"
    );
    let (baseline, baseline_scores) =
        measure_schema(&model, &groups, 1, false, threshold, token_price, true).await;
    let (singleton, singleton_scores) =
        measure(&model, &groups, 1, false, threshold, token_price).await;
    let mut report = json!({
        "dataset_revision": manifest["dataset_revision"], "model": configured_model.as_str(),
        "label_provenance": manifest["label_provenance"],
        "human_reviewed": human_reviewed(&manifest),
        "input_usd_per_million_tokens": token_price,
        "pricing_source": manifest["pricing_source"],
        "singleton_prompt_version": super::super::MATCH_PROMPT_VERSION,
        "batch_prompt_version": BATCH_PROMPT_VERSION,
        "evidence_policy_version": MatchingEvidencePolicy::VERSION,
        "should_show_threshold": threshold, "batch_candidates": batch_size,
        "baseline_two_questions": baseline, "singleton": singleton,
        "schema_comparison": compare(&baseline_scores, &singleton_scores, threshold),
        "rollout_gate": rollout_gate(&manifest, batch_size)
    });
    if batch_size > 1 {
        let (batched, batched_scores) =
            measure(&model, &groups, batch_size, false, threshold, token_price).await;
        let (reversed, reversed_scores) =
            measure(&model, &groups, batch_size, true, threshold, token_price).await;
        report["batched"] = batched;
        report["batched_reversed"] = reversed;
        report["singleton_comparison"] = compare(&singleton_scores, &batched_scores, threshold);
        report["neighbor_order_interference"] =
            compare(&batched_scores, &reversed_scores, threshold);
    }
    // Actual invoiced costs cannot be inferred from successful-response usage alone.
    if let Ok(costs_path) = std::env::var("AURA_BENCH_RECONCILED_COSTS") {
        let costs: Value =
            serde_json::from_slice(&std::fs::read(costs_path).expect("cost reconciliation file"))
                .expect("cost JSON");
        for mode in [
            "baseline_two_questions",
            "singleton",
            "batched",
            "batched_reversed",
        ] {
            if report.get(mode).is_none() {
                continue;
            }
            report[mode]["actual_cost_usd"] = costs[mode]
                .as_f64()
                .filter(|v| v.is_finite() && *v >= 0.0)
                .expect("reconciled per-mode cost")
                .into();
        }
        report["cost_reconciliation_reference"] = required_text(&costs, "reference").into();
    }
    let report_path = std::env::var("AURA_BENCH_REPORT").expect("report destination");
    std::fs::write(report_path, serde_json::to_vec_pretty(&report).unwrap())
        .expect("write benchmark report");
}

#[test]
fn fixture_evidence_preserves_reviewed_text_and_missing_fields() {
    let case = json!({"title": "Antique brass lamp", "description": "Circa 1900"});
    let product = fixture_product(&case, Language::De);
    assert_eq!(case["title"], product.titles[&Language::De].as_ref());
    assert_eq!(
        case["description"],
        product.descriptions[&Language::De].as_ref()
    );
    let missing = fixture_product(&json!({"title": null, "description": null}), Language::En);
    assert!(missing.titles.is_empty());
    assert!(missing.descriptions.is_empty());
    assert!(missing.image.is_none() && missing.images.is_empty());
}

#[test]
fn fixture_evidence_rejects_normalization_and_truncation_before_provider_access() {
    for case in [
        json!({"title": "lowercase title", "description": null}),
        json!({"title": "A".repeat(129), "description": null}),
        json!({"title": null, "description": " trimmed "}),
        json!({"title": null, "description": "A".repeat(4001)}),
    ] {
        assert!(std::panic::catch_unwind(|| fixture_product(&case, Language::En)).is_err());
    }
}

#[test]
fn comparison_reports_disagreement_without_treating_unresolved_scores_as_negatives() {
    let score = |v| Scored {
        score: v,
        label: false,
        fallback: false,
        requested_image_count: 0,
        sent_image_count: Some(0),
    };
    let left = BTreeMap::from([
        ("positive".into(), score(Some(0.8))),
        ("negative".into(), score(Some(0.2))),
        ("missing".into(), score(None)),
    ]);
    let right = BTreeMap::from([
        ("positive".into(), score(Some(0.7))),
        ("negative".into(), score(Some(0.6))),
        ("missing".into(), score(Some(0.9))),
    ]);
    let compared = compare(&left, &right, 0.5);
    assert_eq!(2, compared["compared_candidates"]);
    assert_eq!(0.5, compared["decision_disagreement"]);
    assert_eq!(1, compared["decision_flips"].as_array().unwrap().len());
    assert_eq!("negative", compared["decision_flips"][0]["id"]);
    assert_eq!(
        true,
        compared["decision_flips"][0]["left_correct_right_wrong"]
    );
    assert!(
        (compared["should_show_mean_absolute_delta"]
            .as_f64()
            .unwrap()
            - 0.25)
            .abs()
            < 1e-12
    );
}

#[test]
fn baseline_restores_only_the_old_question_and_diagnostic_metadata() {
    let groups = fixtures(&offline_manifest());
    let mut proposed = groups[0][0].request.clone();
    proposed.image_urls = (0..4)
        .map(|index| url::Url::parse(&format!("https://example.test/{index}.jpg")).unwrap())
        .collect();
    let mut baseline = baseline_request(proposed.clone());
    assert_eq!(2, baseline.questions.len());
    assert_eq!("hard_conflict", baseline.questions[0].id.as_str());
    assert_eq!(proposed.questions[0], baseline.questions[1]);
    assert_eq!(
        true,
        baseline.state["evaluation_context"]["hard_conflict_is_diagnostic_only"]
    );
    baseline.questions.remove(0);
    baseline.state["evaluation_context"]
        .as_object_mut()
        .unwrap()
        .remove("hard_conflict_is_diagnostic_only");
    assert_eq!(proposed, baseline);
    let show = super::super::question_id("should_show");
    let hard = super::super::question_id("hard_conflict");
    let mut response = ClassificationResponse {
        answers: BTreeMap::from([(show, classifier_model::Probability::new(0.5).unwrap())]),
        usage: Default::default(),
        diagnostics: Default::default(),
    };
    assert_eq!(Some(0.5), measured_score(&response, false));
    assert_eq!(None, measured_score(&response, true));
    response
        .answers
        .insert(hard, classifier_model::Probability::new(0.95).unwrap());
    assert_eq!(Some(0.5), measured_score(&response, true));
    assert_eq!(None, measured_score(&response, false));
}

#[test]
fn fixture_images_preserve_reviewed_evidence_and_policy_limits() {
    let mut manifest = offline_manifest();
    for (index, count) in [(0, 0), (1, 1), (2, 4)] {
        manifest["groups"][0]["cases"][index]["image_urls"] = json!(
            (0..count)
                .map(|image| format!("https://example.test/{index}-{image}.jpg"))
                .collect::<Vec<_>>()
        );
    }
    let groups = fixtures(&manifest);
    for (index, count) in [(0, 0), (1, 1), (2, 4)] {
        assert_eq!(count, groups[0][index].request.image_urls.len());
    }
    manifest["groups"][0]["evidence_policy"] = "TEXT_AND_ONE_IMAGE".into();
    manifest["groups"][0]["enhanced_description"] = Value::Null;
    assert!(std::panic::catch_unwind(|| fixtures(&manifest)).is_err());
}

// Synthetic data exercises the runner contract without claiming reviewed model quality.
fn offline_manifest() -> Value {
    let tags = json!([
        "easy_positive",
        "hard_positive",
        "easy_negative",
        "hard_negative",
        "missing_description",
        "missing_images",
        "long_instructions",
        "contradictory_neighbors",
        "injection",
        "antiques",
        "art"
    ]);
    let groups = ["en", "de", "fr"]
        .into_iter()
        .map(|language| {
            json!({
                "language": language,
                "base_queries": ["antique lamp", "bronze sculpture"],
                "enhanced_description": "Original pieces, ideally brass",
                "evidence_policy": "ENHANCED_TEXT_AND_FOUR_IMAGES",
                "cases": (0..14).map(|index| json!({
                    "id": format!("{language}-{index}"),
                    "title": if index % 2 == 0 { "Positive fixture" } else { "Negative fixture" },
                    "description": if index == 1 { None } else { Some("Synthetic evidence") },
                    "human_match": index % 2 == 0,
                    "tags": tags,
                })).collect::<Vec<_>>()
            })
        })
        .collect::<Vec<_>>();
    json!({
        "label_provenance": "human-reviewed", "reviewed_by": "synthetic-test-only",
        "dataset_revision": "offline-contract-test", "batch_candidates": 4, "groups": groups
    })
}

#[test]
fn fixture_manifest_preserves_identity_intent_policy_and_missing_evidence() {
    let manifest = offline_manifest();
    let groups = fixtures(&manifest);
    assert_eq!(3, groups.len());
    assert_eq!(42, groups.iter().map(Vec::len).sum::<usize>());
    let first = &groups[0][0];
    assert_eq!("en-0", first.id);
    assert!(first.label);
    assert_eq!(
        Some("benchmark:en-0"),
        first.request.correlation_id.as_deref()
    );
    assert_eq!(
        manifest["groups"][0]["base_queries"],
        first.request.state["search_intent"]["base_product_queries"]
    );
    assert_eq!(
        manifest["groups"][0]["enhanced_description"],
        first.request.state["search_intent"]["enhanced_description"]
    );
    assert_eq!(
        "ENHANCED_TEXT_AND_FOUR_IMAGES",
        first.request.state["evaluation_context"]["evidence_policy"]
    );
    assert_eq!("", groups[0][1].request.state["candidate"]["description"]);
    assert!(
        groups
            .iter()
            .flatten()
            .all(|fixture| fixture.request.image_urls.is_empty())
    );
}

#[test]
fn fixture_manifest_rejects_untrustworthy_labels_identity_policy_and_coverage() {
    for invalid in [
        "provenance",
        "language",
        "duplicate",
        "policy",
        "enhanced",
        "coverage",
        "size",
        "mixed",
        "sample",
    ] {
        let mut manifest = offline_manifest();
        match invalid {
            "provenance" => manifest["label_provenance"] = "model-generated".into(),
            "language" => manifest["groups"][0]["language"] = "en-US".into(),
            "duplicate" => manifest["groups"][1]["cases"][0]["id"] = "en-0".into(),
            "policy" => manifest["groups"][0]["evidence_policy"] = "UNKNOWN".into(),
            "enhanced" => manifest["groups"][0]["evidence_policy"] = "TEXT_ONLY".into(),
            "coverage" => {
                for group in manifest["groups"].as_array_mut().unwrap() {
                    for case in group["cases"].as_array_mut().unwrap() {
                        case["tags"] = json!([]);
                    }
                }
            }
            "size" => manifest["groups"][0]["cases"] = json!([]),
            "mixed" => {
                for case in manifest["groups"][0]["cases"].as_array_mut().unwrap() {
                    case["human_match"] = false.into();
                }
            }
            "sample" => {
                for group in manifest["groups"].as_array_mut().unwrap() {
                    group["cases"].as_array_mut().unwrap().truncate(4);
                }
            }
            _ => unreachable!(),
        }
        assert!(
            std::panic::catch_unwind(|| fixtures(&manifest)).is_err(),
            "{invalid}"
        );
    }
}

#[tokio::test]
async fn initial_schema_comparison_accepts_hard_cases_and_preserves_the_batch_gate() {
    let mut manifest = offline_manifest();
    let cases = manifest["groups"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|group| group["cases"].as_array().unwrap().iter().cloned())
        .take(30)
        .collect::<Vec<_>>();
    let mut group = manifest["groups"][0].clone();
    group["cases"] = cases.into();
    group["enhanced_description"] = Value::Null;
    group["evidence_policy"] = "TEXT_ONLY".into();
    for case in group["cases"].as_array_mut().unwrap() {
        case["tags"] = json!([]);
    }
    manifest["groups"] = json!([group]);
    manifest["batch_candidates"] = 1.into();
    let groups = fixtures(&manifest);
    let model = MeasuredModel {
        model: OfflineModel(OfflineMode::Complete),
        observations: Mutex::default(),
    };
    let (baseline, left) = measure_schema(&model, &groups, 1, false, 0.5, 2.0, true).await;
    let (proposed, right) = measure(&model, &groups, 1, false, 0.5, 2.0).await;
    for report in [&baseline, &proposed] {
        assert_eq!(30, report["candidates"]);
        assert_eq!(30, report["provider_requests"]);
        assert_eq!(0, report["shared_provider_requests"]);
        assert_eq!(0, report["fallback_requests"]);
    }
    assert_eq!(30, compare(&left, &right, 0.5)["compared_candidates"]);
    manifest["batch_candidates"] = 2.into();
    assert!(std::panic::catch_unwind(|| fixtures(&manifest)).is_err());
}

#[test]
fn agent_authored_fixtures_cannot_claim_the_human_quality_gate() {
    let mut manifest = offline_manifest();
    manifest["label_provenance"] = "agent-authored".into();
    manifest["authored_by"] = "Codex".into();
    manifest.as_object_mut().unwrap().remove("reviewed_by");
    for group in manifest["groups"].as_array_mut().unwrap() {
        for case in group["cases"].as_array_mut().unwrap() {
            let label = case.as_object_mut().unwrap().remove("human_match").unwrap();
            case["expected_match"] = label;
        }
    }
    assert_eq!(42, fixtures(&manifest).iter().map(Vec::len).sum::<usize>());
    assert!(!human_reviewed(&manifest));
    for batch_size in [1, 50] {
        assert_eq!(
            "INELIGIBLE_AGENT_AUTHORED_LABELS",
            rollout_gate(&manifest, batch_size)
        );
    }
    manifest["label_provenance"] = "human-reviewed".into();
    assert!(std::panic::catch_unwind(|| fixtures(&manifest)).is_err());
    let reviewed = offline_manifest();
    assert_eq!("PENDING_HUMAN_SCHEMA_REVIEW", rollout_gate(&reviewed, 1));
    assert_eq!(
        "PENDING_HUMAN_REVIEW_AND_RECONCILED_COSTS",
        rollout_gate(&reviewed, 50)
    );
}

#[test]
fn synthetic_live_fixture_preserves_distinct_cases_and_provisional_labels() {
    let manifest: Value = serde_json::from_str(include_str!(
        "../../../benchmarks/clef-should-show/fixtures.json"
    ))
    .unwrap();
    let groups = fixtures(&manifest);
    assert_eq!(
        vec![50, 20, 20, 10],
        groups.iter().map(Vec::len).collect::<Vec<_>>()
    );
    assert_eq!(
        50,
        groups
            .iter()
            .flatten()
            .filter(|fixture| fixture.label)
            .count()
    );
    assert!(
        groups
            .iter()
            .flatten()
            .all(|fixture| fixture.request.image_urls.is_empty())
    );
    assert_eq!(
        "INELIGIBLE_AGENT_AUTHORED_LABELS",
        rollout_gate(&manifest, 50)
    );
}

#[derive(Clone, Copy)]
enum OfflineMode {
    Complete,
    Partial,
    Timeout,
    InvalidEnvelope,
    UnknownUsage,
    Flipped,
    SchemaShift,
    DegradedImages,
}

struct OfflineModel(OfflineMode);

#[async_trait::async_trait]
impl ClassifierModel for OfflineModel {
    fn request_limits(&self) -> Option<ClassificationRequestLimits> {
        Some(super::tests::limits())
    }

    async fn classify(
        &self,
        request: ClassificationRequest,
    ) -> Result<ClassificationResponse, ClassificationError> {
        if matches!(self.0, OfflineMode::Timeout) {
            return Err(ClassificationError::Timeout);
        }
        let records = request.state["candidates"].as_array();
        if records.is_some() && matches!(self.0, OfflineMode::InvalidEnvelope) {
            return Err(ClassificationError::InvalidResponse);
        }
        let mut answers = BTreeMap::new();
        for question in &request.questions {
            let evidence = if let Some(records) = records {
                let candidate_id = question.id.as_str().split_once('.').unwrap().0;
                if matches!(self.0, OfflineMode::Partial)
                    && candidate_id == records[1]["id"].as_str().unwrap()
                    && question.id.as_str().ends_with(".should_show")
                {
                    continue;
                }
                &records
                    .iter()
                    .find(|record| record["id"] == candidate_id)
                    .unwrap()["evidence"]
            } else {
                &request.state["candidate"]
            };
            let positive = evidence["title"].as_str().unwrap().starts_with("Positive")
                ^ matches!(self.0, OfflineMode::Flipped)
                ^ (matches!(self.0, OfflineMode::SchemaShift)
                    && request
                        .questions
                        .iter()
                        .all(|question| question.id.as_str() != "hard_conflict"));
            answers.insert(
                question.id.clone(),
                classifier_model::Probability::new(if positive { 0.9 } else { 0.1 }).unwrap(),
            );
        }
        Ok(ClassificationResponse {
            answers,
            usage: classifier_model::ClassificationUsage {
                input_tokens: (!matches!(self.0, OfflineMode::UnknownUsage)).then_some(100),
                output_tokens: None,
            },
            diagnostics: classifier_model::ClassificationDiagnostics {
                sent_image_count: if matches!(self.0, OfflineMode::DegradedImages) {
                    request.image_urls.len().saturating_sub(1)
                } else {
                    request.image_urls.len()
                },
                ..Default::default()
            },
        })
    }
}

#[tokio::test]
async fn comparison_keeps_degraded_reviewed_image_evidence_unresolved() {
    let mut manifest = offline_manifest();
    for index in 0..2 {
        manifest["groups"][0]["cases"][index]["image_urls"] =
            json!([format!("https://example.test/{index}.jpg")]);
    }
    let groups = fixtures(&manifest);
    let model = MeasuredModel {
        model: OfflineModel(OfflineMode::DegradedImages),
        observations: Mutex::default(),
    };
    let (baseline, left) = measure_schema(&model, &groups, 1, false, 0.5, 2.0, true).await;
    let (proposed, right) = measure(&model, &groups, 1, false, 0.5, 2.0).await;
    for report in [&baseline, &proposed] {
        assert_eq!(2, report["incomplete_evidence_candidates"]);
        assert_eq!(2, report["unresolved_candidates"]);
        assert_eq!(0, report["false_positive"]);
        assert_eq!(0, report["false_negative"]);
    }
    assert_eq!(40, compare(&left, &right, 0.5)["compared_candidates"]);
}

#[tokio::test]
async fn schema_comparison_reports_every_human_label_regression_and_separate_usage() {
    let groups = fixtures(&offline_manifest());
    let model = MeasuredModel {
        model: OfflineModel(OfflineMode::SchemaShift),
        observations: Mutex::default(),
    };
    let (baseline, left) = measure_schema(&model, &groups, 1, false, 0.5, 2.0, true).await;
    let (proposed, right) = measure(&model, &groups, 1, false, 0.5, 2.0).await;
    assert_eq!(21, baseline["true_positive"]);
    assert_eq!(21, baseline["true_negative"]);
    assert_eq!(21, proposed["false_positive"]);
    assert_eq!(21, proposed["false_negative"]);
    for report in [&baseline, &proposed] {
        assert_eq!(42, report["provider_requests"]);
        assert_eq!(4200, report["known_input_tokens"]);
        assert_eq!(1.0, report["valid_response_rate"]);
        assert_eq!(42, report["scores"].as_array().unwrap().len());
    }
    let comparison = compare(&left, &right, 0.5);
    let flips = comparison["decision_flips"].as_array().unwrap();
    assert_eq!(42, flips.len());
    assert!(
        flips
            .iter()
            .all(|flip| flip["left_correct_right_wrong"] == true)
    );
    assert_eq!(1.0, comparison["decision_disagreement"]);
}

#[tokio::test]
async fn benchmark_accounts_for_shared_calls_recovery_unresolved_work_and_unknown_costs() {
    let groups = fixtures(&offline_manifest());
    for (mode, calls, fallback, failed, unresolved, unknown) in [
        (OfflineMode::Complete, 12, 0, 0, 0, 0),
        (OfflineMode::Partial, 24, 12, 0, 0, 0),
        (OfflineMode::Timeout, 12, 0, 12, 42, 12),
        (OfflineMode::InvalidEnvelope, 54, 42, 12, 0, 12),
        (OfflineMode::UnknownUsage, 12, 0, 0, 0, 12),
    ] {
        let model = MeasuredModel {
            model: OfflineModel(mode),
            observations: Mutex::default(),
        };
        let (report, scores) = measure(&model, &groups, 4, false, 0.5, 2.0).await;
        assert_eq!(42, scores.len());
        assert_eq!(12, report["planned_requests"]);
        assert_eq!(12, report["shared_provider_requests"]);
        assert_eq!(calls, report["provider_requests"]);
        assert_eq!(4, report["largest_request_candidates"]);
        assert_eq!(
            u64::try_from(calls).unwrap(),
            report["request_candidate_counts"]
                .as_object()
                .unwrap()
                .values()
                .map(|count| count.as_u64().unwrap())
                .sum::<u64>()
        );
        assert_eq!(fallback, report["fallback_requests"]);
        assert_eq!(fallback, report["fallback_candidates"]);
        assert_eq!(failed, report["failed_requests"]);
        assert_eq!(unresolved, report["unresolved_candidates"]);
        assert_eq!(unknown, report["unknown_token_requests"]);
        if unresolved == 0 {
            assert_eq!(21, report["true_positive"]);
            assert_eq!(21, report["true_negative"]);
            assert_eq!(1.0, report["recall_including_unresolved"]);
        } else {
            assert_eq!(0, report["false_negative"]);
            assert_eq!(0.0, report["recall_including_unresolved"]);
            assert!(report["precision"].is_null());
        }
        if unknown > 0 {
            assert!(report["token_priced_cost_usd"].is_null());
        } else {
            assert_eq!(calls * 100, report["known_input_tokens"]);
            let expected = f64::from(calls * 100) * 2.0 / 1_000_000.0;
            assert!((report["token_priced_cost_usd"].as_f64().unwrap() - expected).abs() < 1e-12);
        }
        if matches!(mode, OfflineMode::Partial) {
            assert_eq!(12, report["unresolved_questions"]);
        }
        if matches!(mode, OfflineMode::InvalidEnvelope) {
            assert_eq!(12, report["invalid_response_envelopes"]);
        }
    }
}

#[tokio::test]
async fn benchmark_compares_identical_candidate_evidence_across_grouping_and_neighbor_order() {
    let groups = fixtures(&offline_manifest());
    let model = MeasuredModel {
        model: OfflineModel(OfflineMode::Complete),
        observations: Mutex::default(),
    };
    let (singleton, baseline) = measure(&model, &groups, 1, false, 0.5, 2.0).await;
    assert_eq!(42, singleton["provider_requests"]);
    assert_eq!(0, singleton["shared_provider_requests"]);
    let (_, batched) = measure(&model, &groups, 4, false, 0.5, 2.0).await;
    let (_, reversed) = measure(&model, &groups, 4, true, 0.5, 2.0).await;
    assert_eq!(
        0.0,
        compare(&baseline, &batched, 0.5)["decision_disagreement"]
    );
    assert_eq!(
        0.0,
        compare(&batched, &reversed, 0.5)["should_show_mean_absolute_delta"]
    );
    let flipped_model = MeasuredModel {
        model: OfflineModel(OfflineMode::Flipped),
        observations: Mutex::default(),
    };
    let (flipped, scores) = measure(&flipped_model, &groups, 4, false, 0.5, 2.0).await;
    assert_eq!(21, flipped["false_positive"]);
    assert_eq!(21, flipped["false_negative"]);
    assert_eq!(0.0, flipped["precision"]);
    assert_eq!(
        1.0,
        compare(&baseline, &scores, 0.5)["decision_disagreement"]
    );
}

//! Manual, opt-in provider benchmark. Ordinary library validation never calls a live model.
use super::*;
use crate::product_match_evaluator::{match_scores, product_match_request};
use classifier_model::{CloudflareClassifierConfig, CloudflareClassifierModel, CloudflareModel};
use localization::Language;
use product_listing_core::product_listing_search::ProductListingSearch;
use search_filter_core::MatchingEvidencePolicy;
use serde_json::{Value, json};
use std::{collections::BTreeSet, sync::Mutex, time::Instant};

struct Observation {
    millis: u64,
    shared: bool,
    input_tokens: Option<u64>,
    output_tokens: Option<u64>,
    unresolved_questions: usize,
    invalid_envelope: bool,
    failed: bool,
}

struct MeasuredModel {
    model: CloudflareClassifierModel,
    observations: Mutex<Vec<Observation>>,
}

#[async_trait::async_trait]
impl ClassifierModel for MeasuredModel {
    fn request_limits(&self) -> Option<ClassificationRequestLimits> {
        self.model.request_limits()
    }
    async fn classify(
        &self,
        request: ClassificationRequest,
    ) -> Result<ClassificationResponse, ClassificationError> {
        let questions = request.questions.len();
        let shared = request.state.get("candidates").is_some();
        let start = Instant::now();
        let result = self.model.classify(request).await;
        self.observations.lock().unwrap().push(Observation {
            millis: u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX),
            shared,
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
    scores: Option<(f64, f64)>,
    fallback: bool,
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
    product
}

fn fixtures(manifest: &Value) -> Vec<Vec<Fixture>> {
    assert_eq!(
        Some("human-reviewed"),
        manifest["label_provenance"].as_str()
    );
    required_text(manifest, "dataset_revision");
    required_text(manifest, "reviewed_by");
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
            (2..=50).contains(&cases.len()),
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
            let label = case["human_match"].as_bool().expect("human match decision");
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
            let mut request = product_match_request(&product, &search, policy);
            request.correlation_id = Some(format!("benchmark:{id}"));
            group_fixtures.push(Fixture {
                id: id.into(),
                label,
                request,
            });
        }
        assert!(
            group_positive && group_negative,
            "neighbor groups must mix positive and negative evidence"
        );
        groups.push(group_fixtures);
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

async fn measure(
    model: &MeasuredModel,
    groups: &[Vec<Fixture>],
    batch_size: usize,
    reverse: bool,
    threshold: f64,
    token_price: f64,
) -> (Value, BTreeMap<String, Scored>) {
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
        let requests = order.iter().map(|f| f.request.clone()).collect::<Vec<_>>();
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
                    scores: result
                        .result
                        .as_ref()
                        .ok()
                        .and_then(|response| match_scores(response).ok())
                        .map(|(hard, show)| (hard.get(), show.get())),
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
        match scores[&fixture.id].scores {
            Some((_, show)) => match (fixture.label, show >= threshold) {
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
        "fallback_requests": observations.len().saturating_sub(planned_count),
        "fallback_candidates": scores.values().filter(|s| s.fallback).count(),
        "failed_requests": observations.iter().filter(|o| o.failed).count(),
        "unresolved_questions": observations.iter().map(|o| o.unresolved_questions).sum::<usize>(),
        "invalid_response_envelopes": observations.iter().filter(|o| o.invalid_envelope).count(),
        "known_input_tokens": known_tokens, "unknown_token_requests": unknown_token_requests,
        "known_output_tokens": observations.iter().filter_map(|o| o.output_tokens).sum::<u64>(),
        "unknown_output_token_requests": observations.iter().filter(|o| o.output_tokens.is_none()).count(),
        "token_priced_cost_usd": if unknown_token_requests == 0 { Some(known_tokens as f64 * token_price / 1_000_000.0) } else { None },
        "scores": scores.iter().map(|(id, s)| json!({"id": id, "hard_conflict": s.scores.map(|s| s.0), "should_show": s.scores.map(|s| s.1)})).collect::<Vec<_>>()
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
    let mut hard_delta = 0.0;
    let mut max_show_delta: f64 = 0.0;
    for (id, left) in left {
        if let (Some((left_hard, left_show)), Some((right_hard, right_show))) =
            (left.scores, right[id].scores)
        {
            compared += 1;
            disagreement += usize::from((left_show >= threshold) != (right_show >= threshold));
            let delta = (left_show - right_show).abs();
            show_delta += delta;
            hard_delta += (left_hard - right_hard).abs();
            max_show_delta = max_show_delta.max(delta);
        }
    }
    json!({"compared_candidates": compared, "decision_disagreement": ratio(disagreement, compared),
        "should_show_mean_absolute_delta": (compared > 0).then(|| show_delta / compared as f64),
        "hard_conflict_mean_absolute_delta": (compared > 0).then(|| hard_delta / compared as f64),
        "should_show_max_absolute_delta": max_show_delta})
}

/// Requires a reviewed fixture and dedicated test account. Writes only aggregate
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
        .filter(|v| (0.0..=1.0).contains(v))
        .expect("acceptance threshold");
    let batch_size = manifest["batch_candidates"]
        .as_u64()
        .filter(|v| (2..=32).contains(v))
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
        groups.iter().any(|group| {
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
    let (singleton, singleton_scores) =
        measure(&model, &groups, 1, false, threshold, token_price).await;
    let (batched, batched_scores) =
        measure(&model, &groups, batch_size, false, threshold, token_price).await;
    let (reversed, reversed_scores) =
        measure(&model, &groups, batch_size, true, threshold, token_price).await;
    let mut report = json!({
        "dataset_revision": manifest["dataset_revision"], "model": configured_model.as_str(),
        "singleton_prompt_version": super::super::MATCH_PROMPT_VERSION,
        "batch_prompt_version": BATCH_PROMPT_VERSION,
        "evidence_policy_version": MatchingEvidencePolicy::VERSION,
        "should_show_threshold": threshold, "batch_candidates": batch_size,
        "singleton": singleton, "batched": batched, "batched_reversed": reversed,
        "singleton_comparison": compare(&singleton_scores, &batched_scores, threshold),
        "neighbor_order_interference": compare(&batched_scores, &reversed_scores, threshold),
        "rollout_gate": "PENDING_HUMAN_REVIEW_AND_RECONCILED_COSTS"
    });
    // Actual invoiced costs cannot be inferred from successful-response usage alone.
    if let Ok(costs_path) = std::env::var("AURA_BENCH_RECONCILED_COSTS") {
        let costs: Value =
            serde_json::from_slice(&std::fs::read(costs_path).expect("cost reconciliation file"))
                .expect("cost JSON");
        for mode in ["singleton", "batched", "batched_reversed"] {
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
        scores: v,
        fallback: false,
    };
    let left = BTreeMap::from([
        ("positive".into(), score(Some((0.1, 0.8)))),
        ("negative".into(), score(Some((0.9, 0.2)))),
        ("missing".into(), score(None)),
    ]);
    let right = BTreeMap::from([
        ("positive".into(), score(Some((0.2, 0.7)))),
        ("negative".into(), score(Some((0.8, 0.6)))),
        ("missing".into(), score(Some((0.1, 0.9)))),
    ]);
    let compared = compare(&left, &right, 0.5);
    assert_eq!(2, compared["compared_candidates"]);
    assert_eq!(0.5, compared["decision_disagreement"]);
    assert!(
        (compared["should_show_mean_absolute_delta"]
            .as_f64()
            .unwrap()
            - 0.25)
            .abs()
            < 1e-12
    );
}

use once_cell::sync::Lazy;
use serde::Deserialize;

/// Soft intent classification of a search query.
///
/// Probabilities always sum to 1.0 (after softmax normalisation).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct IntentSignals {
    /// Lookup queries with concrete identifiers (e.g. years, brand, model).
    pub precision_score: f32,
    /// Period-/style-driven queries (e.g. "art deco lamp").
    pub style_score: f32,
    /// Visually descriptive queries (colour, shape, material appearance).
    pub visual_score: f32,
    /// Vague, browsing-style queries.
    pub exploratory_score: f32,
}

impl IntentSignals {
    pub fn uniform() -> Self {
        Self {
            precision_score: 0.25,
            style_score: 0.25,
            visual_score: 0.25,
            exploratory_score: 0.25,
        }
    }
}

#[derive(Debug, Deserialize)]
struct IntentExampleData {
    #[allow(dead_code)]
    query: String,
    embedding: Vec<f32>,
}

#[derive(Debug, Deserialize)]
struct IntentBucket {
    #[allow(dead_code)]
    description: String,
    examples: Vec<IntentExampleData>,
}

#[derive(Debug, Deserialize)]
struct IntentExamplesFile {
    precision: IntentBucket,
    style: IntentBucket,
    visual: IntentBucket,
    exploratory: IntentBucket,
}

/// Pre-computed centroids (mean vector) per intent, loaded from
/// `src/product/data/intent_examples.json`.
#[derive(Debug, Clone)]
pub struct IntentCentroids {
    pub precision: Vec<f32>,
    pub style: Vec<f32>,
    pub visual: Vec<f32>,
    pub exploratory: Vec<f32>,
}

static INTENT_EXAMPLES_RAW: &str = include_str!("../../data/intent_examples.json");

static INTENT_CENTROIDS: Lazy<IntentCentroids> = Lazy::new(|| {
    let parsed: IntentExamplesFile = serde_json::from_str(INTENT_EXAMPLES_RAW)
        .expect("intent_examples.json must be valid IntentExamplesFile");
    IntentCentroids {
        precision: centroid(&parsed.precision.examples),
        style: centroid(&parsed.style.examples),
        visual: centroid(&parsed.visual.examples),
        exploratory: centroid(&parsed.exploratory.examples),
    }
});

fn centroid(examples: &[IntentExampleData]) -> Vec<f32> {
    if examples.is_empty() {
        return Vec::new();
    }
    let dim = examples[0].embedding.len();
    let mut sum = vec![0f32; dim];
    let mut counted = 0u32;
    for ex in examples {
        if ex.embedding.len() != dim {
            continue;
        }
        for (s, v) in sum.iter_mut().zip(ex.embedding.iter()) {
            *s += *v;
        }
        counted += 1;
    }
    if counted == 0 {
        return sum;
    }
    let n = counted as f32;
    for s in &mut sum {
        *s /= n;
    }
    // Unit-normalise so cosine similarity becomes a plain dot product.
    let norm: f32 = sum.iter().map(|v| v * v).sum::<f32>().sqrt();
    if norm > 0.0 {
        for s in &mut sum {
            *s /= norm;
        }
    }
    sum
}

/// Returns the lazily-loaded intent centroids.
pub fn intent_centroids() -> &'static IntentCentroids {
    &INTENT_CENTROIDS
}

/// Compute soft intent signals from raw inputs.
///
/// Inputs:
/// - `query`: the raw user query (used for keyword heuristics).
/// - `query_embedding`: unit-normalised embedding of the query, or `None`.
///   When provided, contributes a centroid-similarity signal per intent.
/// - `bm25_scores`: BM25 scores of the top-N hits for the query, ordered descending.
///   When non-empty, contributes a peakedness signal (peaked → precision, flat → exploratory).
/// - `centroids`: intent centroids (typically `intent_centroids()`).
pub fn compute_intent_signals(
    query: &str,
    query_embedding: Option<&[f32]>,
    bm25_scores: &[f32],
    centroids: &IntentCentroids,
) -> IntentSignals {
    let centroid_sims = match query_embedding {
        Some(qe) => CentroidSimilarities {
            precision: cosine(qe, &centroids.precision),
            style: cosine(qe, &centroids.style),
            visual: cosine(qe, &centroids.visual),
            exploratory: cosine(qe, &centroids.exploratory),
        },
        None => CentroidSimilarities::default(),
    };

    let kw = keyword_scores(query);
    let bm25 = bm25_distribution_score(bm25_scores);

    // Compose raw logits per intent. Weights are deliberately tuned so that with
    // placeholder zero embeddings (centroid_sims == 0) the keyword + bm25 signals
    // still produce sensible behaviour.
    let precision_logit =
        1.5 * centroid_sims.precision + 1.2 * kw.precision + 0.8 * bm25.peakedness;
    let style_logit = 1.5 * centroid_sims.style + 1.0 * kw.style;
    let visual_logit = 1.5 * centroid_sims.visual + 1.0 * kw.visual;
    let exploratory_logit =
        1.5 * centroid_sims.exploratory + 1.0 * kw.exploratory + 0.6 * bm25.flatness;

    let logits = [
        precision_logit,
        style_logit,
        visual_logit,
        exploratory_logit,
    ];
    let probs = softmax(&logits);

    IntentSignals {
        precision_score: probs[0],
        style_score: probs[1],
        visual_score: probs[2],
        exploratory_score: probs[3],
    }
}

#[derive(Debug, Default, Clone, Copy)]
struct CentroidSimilarities {
    precision: f32,
    style: f32,
    visual: f32,
    exploratory: f32,
}

#[derive(Debug, Default, Clone, Copy)]
struct KeywordScores {
    precision: f32,
    style: f32,
    visual: f32,
    exploratory: f32,
}

fn cosine(a: &[f32], b: &[f32]) -> f32 {
    if a.is_empty() || b.is_empty() || a.len() != b.len() {
        return 0.0;
    }
    a.iter().zip(b.iter()).map(|(x, y)| x * y).sum()
}

fn softmax(logits: &[f32]) -> Vec<f32> {
    if logits.is_empty() {
        return Vec::new();
    }
    let max = logits.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
    let exps: Vec<f32> = logits.iter().map(|l| (*l - max).exp()).collect();
    let sum: f32 = exps.iter().sum();
    if sum == 0.0 || !sum.is_finite() {
        let n = logits.len() as f32;
        return vec![1.0 / n; logits.len()];
    }
    exps.into_iter().map(|e| e / sum).collect()
}

/// Quick keyword heuristics for query intent. Lower-case substring/regex matches.
fn keyword_scores(query: &str) -> KeywordScores {
    let q = query.to_lowercase();
    let token_count = q.split_whitespace().count();

    // Years (3-4 digit numbers, optional BC/AD), explicit material grams etc → precision
    let has_year = regex_year().is_match(&q);
    let has_long_digit = q.chars().filter(|c| c.is_ascii_digit()).count() >= 3;
    // Heuristic: a 3+ character token starting with an uppercase letter is treated as a
    // brand-like signal. This is intentionally permissive — it produces some false positives
    // for sentence-initial words like "The" but biases the score by a small amount and is
    // backed up by the year/digit signals before precision overtakes other intents.
    let has_uppercase_brand = query
        .split_whitespace()
        .any(|tok| tok.len() >= 3 && tok.chars().next().is_some_and(|c| c.is_uppercase()));

    let style_keywords = [
        "art deco",
        "art nouveau",
        "baroque",
        "rococo",
        "victorian",
        "georgian",
        "empire",
        "biedermeier",
        "jugendstil",
        "mid-century",
        "mid century",
        "modernist",
        "minimalist",
        "industrial",
        "neoclassical",
    ];
    let visual_keywords = [
        "blue",
        "red",
        "green",
        "yellow",
        "black",
        "white",
        "gold",
        "silver",
        "bronze",
        "brass",
        "wooden",
        "ceramic",
        "porcelain",
        "glass",
        "ornate",
        "carved",
        "engraved",
        "patterned",
        "round",
        "square",
        "small",
        "large",
        "tiny",
        "huge",
    ];
    let exploratory_keywords = [
        "antique",
        "vintage",
        "old",
        "stuff",
        "things",
        "collectibles",
        "rare",
        "decorations",
        "decor",
        "interesting",
    ];

    let style_hits = style_keywords.iter().filter(|k| q.contains(*k)).count() as f32;
    let visual_hits = visual_keywords
        .iter()
        .filter(|k| q.split_whitespace().any(|t| t == **k))
        .count() as f32;
    let exploratory_hits = exploratory_keywords
        .iter()
        .filter(|k| q.split_whitespace().any(|t| t == **k))
        .count() as f32;

    let mut precision: f32 = 0.0;
    if has_year {
        precision += 1.0;
    }
    if has_long_digit {
        precision += 0.4;
    }
    if has_uppercase_brand {
        precision += 0.6;
    }

    let mut exploratory: f32 = 0.0;
    if token_count <= 2 {
        exploratory += 0.5;
    }
    exploratory += 0.4 * exploratory_hits;

    KeywordScores {
        precision: precision.min(2.0),
        style: (0.6 * style_hits).min(2.0),
        visual: (0.4 * visual_hits).min(2.0),
        exploratory: exploratory.min(2.0),
    }
}

fn regex_year() -> &'static regex::Regex {
    static R: Lazy<regex::Regex> = Lazy::new(|| regex::Regex::new(r"\b\d{3,4}\b").unwrap());
    &R
}

/// Peakedness vs flatness of a top-N BM25 score list.
///
/// - `peakedness ≈ 1.0` when the top hit dominates strongly (precision-friendly distribution).
/// - `flatness ≈ 1.0` when scores are roughly uniform (exploratory-friendly distribution).
#[derive(Debug, Default, Clone, Copy)]
struct Bm25Distribution {
    peakedness: f32,
    flatness: f32,
}

fn bm25_distribution_score(scores: &[f32]) -> Bm25Distribution {
    if scores.len() < 2 {
        return Bm25Distribution::default();
    }
    let top = scores[0].max(0.0);
    if top == 0.0 {
        return Bm25Distribution::default();
    }
    let mut sorted: Vec<f32> = scores.iter().map(|s| s.max(0.0)).collect();
    sorted.sort_by(|a, b| b.partial_cmp(a).unwrap_or(std::cmp::Ordering::Equal));
    let median = sorted[sorted.len() / 2];
    // Normalised gap between top and median, in [0, 1].
    let gap = ((top - median) / top).clamp(0.0, 1.0);
    Bm25Distribution {
        peakedness: gap,
        flatness: 1.0 - gap,
    }
}

/// Adaptive search parameters derived from the intent signals.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HybridSearchParams {
    /// Weight applied to the kNN ranking when fusing with BM25. In `[0.0, 0.8]` to honour
    /// the guardrail that BM25 must keep `>= 0.2` influence and we never go pure-vector.
    pub vector_weight: f32,
    /// Number of candidates to fetch from each retriever (BM25 and kNN).
    pub candidate_k: u16,
}

impl HybridSearchParams {
    pub const MIN_CANDIDATE_K: u16 = 200;
    pub const MAX_CANDIDATE_K: u16 = 3000;
    pub const MIN_BM25_WEIGHT: f32 = 0.2;

    /// Derive parameters from soft intent signals.
    ///
    /// Formula matches the issue's recommended shape; clamped per the guardrails.
    pub fn from_intent(signals: &IntentSignals) -> Self {
        let raw_vector_weight = 0.2 * signals.precision_score
            + 0.5 * signals.style_score
            + 0.7 * signals.visual_score
            + 0.8 * signals.exploratory_score;
        let vector_weight = raw_vector_weight.clamp(0.0, 1.0 - Self::MIN_BM25_WEIGHT);

        let raw_k = 200.0 * signals.precision_score
            + 800.0 * signals.style_score
            + 1500.0 * signals.visual_score
            + 3000.0 * signals.exploratory_score;
        let candidate_k = (raw_k.round() as i32)
            .clamp(Self::MIN_CANDIDATE_K as i32, Self::MAX_CANDIDATE_K as i32)
            as u16;

        Self {
            vector_weight,
            candidate_k,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_load_intent_centroids_from_data_file() {
        let c = intent_centroids();
        // Placeholder embeddings are zero, so centroids are zero-length-normalised → all zero.
        assert_eq!(c.precision.len(), 768);
        assert_eq!(c.style.len(), 768);
        assert_eq!(c.visual.len(), 768);
        assert_eq!(c.exploratory.len(), 768);
    }

    #[test]
    fn should_softmax_to_one() {
        let p = softmax(&[1.0, 1.0, 1.0, 1.0]);
        assert!((p.iter().sum::<f32>() - 1.0).abs() < 1e-5);
        for v in p {
            assert!((v - 0.25).abs() < 1e-5);
        }
    }

    #[test]
    fn should_score_uniform_when_query_is_neutral() {
        // Neutral query: 4 tokens (no short-query bonus) and no keyword hits in any bucket.
        let signals =
            compute_intent_signals("the unique piece offering", None, &[], intent_centroids());
        // No keyword hits, no embedding, no bm25 distribution → uniform after softmax.
        let total = signals.precision_score
            + signals.style_score
            + signals.visual_score
            + signals.exploratory_score;
        assert!((total - 1.0).abs() < 1e-5);
        assert!((signals.precision_score - 0.25).abs() < 1e-5);
    }

    #[test]
    fn should_skew_towards_precision_when_query_contains_year_and_brand() {
        let signals = compute_intent_signals(
            "Rolex Submariner 1965",
            None,
            &[12.0, 0.5, 0.4, 0.3, 0.2],
            intent_centroids(),
        );
        assert!(
            signals.precision_score > signals.exploratory_score,
            "precision={} exploratory={}",
            signals.precision_score,
            signals.exploratory_score
        );
        assert!(signals.precision_score > signals.visual_score);
    }

    #[test]
    fn should_skew_towards_style_when_query_contains_period_keyword() {
        let signals = compute_intent_signals("art deco floor lamp", None, &[], intent_centroids());
        assert!(signals.style_score > signals.precision_score);
        assert!(signals.style_score > signals.exploratory_score);
    }

    #[test]
    fn should_skew_towards_visual_when_query_contains_color_and_material() {
        let signals =
            compute_intent_signals("blue ceramic ornate vase", None, &[], intent_centroids());
        assert!(signals.visual_score > signals.precision_score);
        assert!(signals.visual_score > signals.style_score);
    }

    #[test]
    fn should_skew_towards_exploratory_when_query_is_short_and_vague() {
        let signals = compute_intent_signals(
            "antique stuff",
            None,
            &[1.0, 0.95, 0.9, 0.88, 0.85],
            intent_centroids(),
        );
        assert!(signals.exploratory_score > signals.precision_score);
    }

    #[test]
    fn should_clamp_vector_weight_to_at_most_eighty_percent() {
        let signals = IntentSignals {
            precision_score: 0.0,
            style_score: 0.0,
            visual_score: 0.0,
            exploratory_score: 1.0,
        };
        let params = HybridSearchParams::from_intent(&signals);
        assert!(
            params.vector_weight <= 1.0 - HybridSearchParams::MIN_BM25_WEIGHT + 1e-6,
            "vector_weight={} must respect guardrail",
            params.vector_weight
        );
        // BM25 must keep >= 0.2 influence ⟹ vector_weight <= 0.8
        assert!(params.vector_weight <= 0.8 + 1e-6);
    }

    #[test]
    fn should_clamp_candidate_k_to_window() {
        let signals = IntentSignals {
            precision_score: 1.0,
            style_score: 0.0,
            visual_score: 0.0,
            exploratory_score: 0.0,
        };
        let params = HybridSearchParams::from_intent(&signals);
        assert_eq!(params.candidate_k, HybridSearchParams::MIN_CANDIDATE_K);

        let signals = IntentSignals {
            precision_score: 0.0,
            style_score: 0.0,
            visual_score: 0.0,
            exploratory_score: 1.0,
        };
        let params = HybridSearchParams::from_intent(&signals);
        assert_eq!(params.candidate_k, HybridSearchParams::MAX_CANDIDATE_K);
    }

    #[test]
    fn should_grow_vector_weight_with_exploratory_share() {
        let mostly_precision = IntentSignals {
            precision_score: 0.9,
            style_score: 0.05,
            visual_score: 0.025,
            exploratory_score: 0.025,
        };
        let mostly_exploratory = IntentSignals {
            precision_score: 0.05,
            style_score: 0.05,
            visual_score: 0.1,
            exploratory_score: 0.8,
        };
        let p1 = HybridSearchParams::from_intent(&mostly_precision);
        let p2 = HybridSearchParams::from_intent(&mostly_exploratory);
        assert!(p2.vector_weight > p1.vector_weight);
        assert!(p2.candidate_k > p1.candidate_k);
    }

    #[test]
    fn should_compute_bm25_peakedness_for_dominant_top_hit() {
        let dist = bm25_distribution_score(&[20.0, 1.0, 0.9, 0.8]);
        assert!(dist.peakedness > dist.flatness);

        let flat = bm25_distribution_score(&[1.0, 0.99, 0.98, 0.97]);
        assert!(flat.flatness > flat.peakedness);
    }
}

//! Reviewed description boilerplate: candidate analysis over current raw descriptions and the
//! import of reviewer decisions. See `docs/product-listing.md#description-boilerplate`.
//!
//! Analysis counts, per listing source, the listings whose latest raw description contains each
//! sentence. A sentence repeated across enough of a source's listings is a candidate until a
//! reviewer decides it: `STRIP` for shop boilerplate, `KEEP` for an item fact such as whether a
//! postcard was posted. Candidate records carry listing text and belong in a private working
//! file, never in logs or the repository.

use crate::write_record;
use listing_source_core::ListingSourceId;
use product_listing_normalization::{
    BoilerplateSentence, DescriptionBoilerplate, description_sentences,
};
use product_service::ports::{
    CurrentRawDescription, CurrentRawDescriptionCursor, CurrentRawDescriptionReader,
    DescriptionBoilerplateDecision, DescriptionBoilerplateReader, DescriptionBoilerplateReview,
    DescriptionBoilerplateReviewWriter,
};
use serde_json::{Value, json};
use std::collections::{BTreeSet, HashMap};
use std::io::Write;

pub const DEFAULT_MIN_LISTINGS: u64 = 5;
pub const DEFAULT_MIN_SHARE: f64 = 0.2;

/// How often a sentence must repeat within one source to be proposed.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CandidateThresholds {
    pub min_listings: u64,
    /// Share of the source's analysed listings, in `0.0..=1.0`.
    pub min_share: f64,
}

impl Default for CandidateThresholds {
    fn default() -> Self {
        Self {
            min_listings: DEFAULT_MIN_LISTINGS,
            min_share: DEFAULT_MIN_SHARE,
        }
    }
}

impl CandidateThresholds {
    fn repeated(&self, listings: u64, source_listings: u64) -> bool {
        listings >= self.min_listings
            && source_listings > 0
            && listings as f64 / source_listings as f64 >= self.min_share
    }
}

/// The reviews of one listing source: its `STRIP` set and every sentence already decided.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SourceReviews {
    pub strip: DescriptionBoilerplate,
    pub reviewed: BTreeSet<BoilerplateSentence>,
}

impl SourceReviews {
    fn by_source(
        reviews: Vec<DescriptionBoilerplateReview>,
    ) -> HashMap<ListingSourceId, SourceReviews> {
        let mut grouped = HashMap::<ListingSourceId, (Vec<_>, BTreeSet<_>)>::new();
        for review in reviews {
            let (strip, reviewed) = grouped.entry(review.listing_source_id).or_default();
            if review.decision == DescriptionBoilerplateDecision::Strip {
                strip.push(review.sentence.clone());
            }
            reviewed.insert(review.sentence);
        }
        grouped
            .into_iter()
            .map(|(source, (strip, reviewed))| {
                (
                    source,
                    SourceReviews {
                        strip: strip.into_iter().collect(),
                        reviewed,
                    },
                )
            })
            .collect()
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct SentenceCount {
    listings: u64,
    /// Bytes of every occurrence, for the source's repeated share.
    bytes: u64,
    /// The first occurrence as written, for the reviewer.
    example: String,
}

/// Sentence counts of one listing source's current descriptions.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SourceAnalysis {
    listings: u64,
    unreadable_listings: u64,
    description_bytes: u64,
    stripped_bytes: u64,
    sentences: HashMap<BoilerplateSentence, SentenceCount>,
}

impl SourceAnalysis {
    /// Counts one listing's description. `None` is a retained value normalization rejects.
    pub fn add_listing(&mut self, fragments: Option<&[String]>, strip: &DescriptionBoilerplate) {
        let Some(fragments) = fragments else {
            self.unreadable_listings += 1;
            return;
        };
        self.listings += 1;
        let mut seen = BTreeSet::new();
        for sentence in fragments
            .iter()
            .flat_map(|text| description_sentences(text))
        {
            let bytes = sentence.len() as u64;
            self.description_bytes += bytes;
            let Ok(key) = BoilerplateSentence::new(sentence) else {
                continue;
            };
            let count = self.sentences.entry(key.clone()).or_default();
            count.bytes += bytes;
            if count.example.is_empty() {
                count.example = sentence.to_owned();
            }
            if seen.insert(key) {
                count.listings += 1;
            }
        }
        let kept_bytes: u64 = strip
            .strip(fragments.to_vec())
            .iter()
            .flat_map(|text| description_sentences(text))
            .map(|sentence| sentence.len() as u64)
            .sum();
        let listing_bytes: u64 = fragments
            .iter()
            .flat_map(|text| description_sentences(text))
            .map(|sentence| sentence.len() as u64)
            .sum();
        self.stripped_bytes += listing_bytes - kept_bytes;
    }

    /// Candidate records, most repeated first, followed by the source record.
    pub fn records(
        &self,
        listing_source_id: ListingSourceId,
        reviews: &SourceReviews,
        thresholds: CandidateThresholds,
    ) -> Vec<Value> {
        let mut repeated = self
            .sentences
            .iter()
            .filter(|(_, count)| thresholds.repeated(count.listings, self.listings))
            .collect::<Vec<_>>();
        repeated.sort_by(|(left_key, left), (right_key, right)| {
            right
                .listings
                .cmp(&left.listings)
                .then_with(|| left_key.cmp(right_key))
        });
        let repeated_bytes: u64 = repeated.iter().map(|(_, count)| count.bytes).sum();
        let mut records = repeated
            .into_iter()
            .filter(|(sentence, _)| !reviews.reviewed.contains(*sentence))
            .map(|(sentence, count)| {
                json!({
                    "type": "candidate",
                    "listingSourceId": listing_source_id.to_string(),
                    "sentence": sentence.as_str(),
                    "example": count.example,
                    "listings": count.listings,
                    "share": share(count.listings, self.listings),
                })
            })
            .collect::<Vec<_>>();
        let candidates = records.len();
        records.push(json!({
            "type": "source",
            "listingSourceId": listing_source_id.to_string(),
            "listings": self.listings,
            "unreadableListings": self.unreadable_listings,
            "descriptionBytes": self.description_bytes,
            "repeatedBytes": repeated_bytes,
            "repeatedShare": share(repeated_bytes, self.description_bytes),
            "strippedBytes": self.stripped_bytes,
            "strippedShare": share(self.stripped_bytes, self.description_bytes),
            "stripSentences": reviews.strip.sentences().count(),
            "candidates": candidates,
        }));
        records
    }
}

fn share(part: u64, whole: u64) -> f64 {
    if whole == 0 {
        return 0.0;
    }
    (part as f64 / whole as f64 * 10_000.0).round() / 10_000.0
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CandidateCompletion {
    pub sources: u64,
    pub listings: u64,
    pub candidates: u64,
    pub error: Option<String>,
}

impl CandidateCompletion {
    pub fn to_json(&self) -> Value {
        json!({
            "type": "candidatesSummary",
            "completed": self.error.is_none(),
            "error": self.error,
            "sources": self.sources,
            "listings": self.listings,
            "candidates": self.candidates,
        })
    }
}

/// Analyses the latest raw description of every listing of the selected source, or of every
/// source, one source at a time, and writes candidate and source records.
pub async fn boilerplate_candidates(
    descriptions: &dyn CurrentRawDescriptionReader,
    boilerplate: &dyn DescriptionBoilerplateReader,
    listing_source_id: Option<ListingSourceId>,
    thresholds: CandidateThresholds,
    page_size: u32,
    out: &mut dyn Write,
) -> CandidateCompletion {
    let mut completion = CandidateCompletion::default();
    let reviews = match boilerplate.reviews(listing_source_id).await {
        Ok(reviews) => SourceReviews::by_source(reviews),
        Err(error) => {
            completion.error = Some(format!("failed to read boilerplate reviews: {error}"));
            return completion;
        }
    };
    let page_size = page_size.max(1);
    let no_reviews = SourceReviews::default();
    let mut current: Option<(ListingSourceId, SourceAnalysis)> = None;
    let mut after = None;
    loop {
        let page = match descriptions.page(listing_source_id, after, page_size).await {
            Ok(page) => page,
            Err(error) => {
                completion.error = Some(format!("failed to read raw descriptions: {error}"));
                return completion;
            }
        };
        let page_len = page.len();
        for CurrentRawDescription {
            listing_source_id,
            product_listing_raw_stream_id,
            fragments,
        } in page
        {
            after = Some(CurrentRawDescriptionCursor {
                listing_source_id,
                product_listing_raw_stream_id,
            });
            if current
                .as_ref()
                .is_some_and(|(source, _)| *source != listing_source_id)
                && let Some((source, analysis)) = current.take()
                && let Err(error) = finish_source(
                    source,
                    &analysis,
                    reviews.get(&source).unwrap_or(&no_reviews),
                    thresholds,
                    &mut completion,
                    out,
                )
            {
                completion.error = Some(error);
                return completion;
            }
            let (_, analysis) =
                current.get_or_insert_with(|| (listing_source_id, SourceAnalysis::default()));
            let strip = &reviews.get(&listing_source_id).unwrap_or(&no_reviews).strip;
            analysis.add_listing(fragments.as_deref(), strip);
        }
        if page_len < page_size as usize {
            break;
        }
    }
    if let Some((source, analysis)) = current
        && let Err(error) = finish_source(
            source,
            &analysis,
            reviews.get(&source).unwrap_or(&no_reviews),
            thresholds,
            &mut completion,
            out,
        )
    {
        completion.error = Some(error);
    }
    completion
}

fn finish_source(
    listing_source_id: ListingSourceId,
    analysis: &SourceAnalysis,
    reviews: &SourceReviews,
    thresholds: CandidateThresholds,
    completion: &mut CandidateCompletion,
    out: &mut dyn Write,
) -> Result<(), String> {
    let records = analysis.records(listing_source_id, reviews, thresholds);
    completion.sources += 1;
    completion.listings += analysis.listings;
    completion.candidates += records.len() as u64 - 1;
    records
        .iter()
        .try_for_each(|record| write_record(out, record))
}

/// Parses reviewer decisions: one JSON object per line with `listingSourceId`, `sentence` and
/// `decision` (`STRIP` or `KEEP`). Candidate records with an added `decision` qualify; `source`
/// and summary records are skipped. Any other line without a valid decision fails the whole
/// file, so an undecided candidate is never imported by accident.
pub fn parse_reviews(
    input: impl std::io::BufRead,
) -> Result<Vec<DescriptionBoilerplateReview>, String> {
    let mut reviews = Vec::new();
    for (index, line) in input.lines().enumerate() {
        let line_number = index + 1;
        let line = line.map_err(|error| format!("line {line_number}: {error}"))?;
        if line.trim().is_empty() {
            continue;
        }
        let record: Value = serde_json::from_str(&line)
            .map_err(|error| format!("line {line_number}: not JSON: {error}"))?;
        if matches!(
            record.get("type").and_then(Value::as_str),
            Some("source" | "candidatesSummary")
        ) {
            continue;
        }
        let text = |name: &str| {
            record
                .get(name)
                .and_then(Value::as_str)
                .ok_or_else(|| format!("line {line_number}: `{name}` is missing"))
        };
        let listing_source_id = text("listingSourceId")?
            .parse::<ListingSourceId>()
            .map_err(|error| format!("line {line_number}: invalid listing source: {error}"))?;
        let sentence = BoilerplateSentence::new(text("sentence")?)
            .map_err(|error| format!("line {line_number}: {error}"))?;
        let decision = DescriptionBoilerplateDecision::from_code(text("decision")?)
            .ok_or_else(|| format!("line {line_number}: decision must be STRIP or KEEP"))?;
        reviews.push(DescriptionBoilerplateReview {
            listing_source_id,
            sentence,
            decision,
        });
    }
    Ok(reviews)
}

/// Records the reviews of one file atomically.
pub async fn import_reviews(
    writer: &dyn DescriptionBoilerplateReviewWriter,
    reviews: Vec<DescriptionBoilerplateReview>,
) -> Result<usize, String> {
    let count = reviews.len();
    writer
        .record(reviews)
        .await
        .map_err(|error| format!("failed to record boilerplate reviews: {error}"))?;
    Ok(count)
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use product_listing_core::product_listing_raw_id::ProductListingRawStreamId;
    use product_service::ports::DescriptionBoilerplatePortError;

    const NOTICE: &str = "Free shipping has no tracking number.";

    fn sentence(text: &str) -> BoilerplateSentence {
        BoilerplateSentence::new(text).unwrap()
    }

    fn review(
        listing_source_id: ListingSourceId,
        text: &str,
        decision: DescriptionBoilerplateDecision,
    ) -> DescriptionBoilerplateReview {
        DescriptionBoilerplateReview {
            listing_source_id,
            sentence: sentence(text),
            decision,
        }
    }

    fn listing(text: &str) -> Vec<String> {
        vec![text.to_owned()]
    }

    #[test]
    fn should_propose_sentences_repeated_across_a_source_once_per_listing() {
        let source = ListingSourceId::new();
        let mut analysis = SourceAnalysis::default();
        let strip = DescriptionBoilerplate::new();
        for item in ["Chair.", "Table.", "Lamp."] {
            analysis.add_listing(Some(&listing(&format!("{item} {NOTICE} {NOTICE}"))), &strip);
        }
        analysis.add_listing(Some(&listing("Clock.")), &strip);
        analysis.add_listing(None, &strip);

        let records = analysis.records(
            source,
            &SourceReviews::default(),
            CandidateThresholds {
                min_listings: 2,
                min_share: 0.5,
            },
        );

        assert_eq!(2, records.len());
        assert_eq!(
            json!("free shipping has no tracking number."),
            records[0]["sentence"]
        );
        assert_eq!(json!(NOTICE), records[0]["example"]);
        assert_eq!(json!(3), records[0]["listings"]);
        assert_eq!(json!(0.75), records[0]["share"]);
        assert_eq!(json!("source"), records[1]["type"]);
        assert_eq!(json!(4), records[1]["listings"]);
        assert_eq!(json!(1), records[1]["unreadableListings"]);
        assert_eq!(json!(NOTICE.len() * 6), records[1]["repeatedBytes"]);
        assert_eq!(json!(0), records[1]["strippedBytes"]);
    }

    #[test]
    fn should_not_propose_reviewed_sentences_again_and_measure_what_strip_removes() {
        let source = ListingSourceId::new();
        let posted = "Gelaufen.";
        let reviews = SourceReviews::by_source(vec![
            review(source, NOTICE, DescriptionBoilerplateDecision::Strip),
            review(source, posted, DescriptionBoilerplateDecision::Keep),
        ])
        .remove(&source)
        .unwrap();
        let mut analysis = SourceAnalysis::default();
        for _ in 0..5 {
            analysis.add_listing(
                Some(&listing(&format!("Ansichtskarte. {posted} {NOTICE}"))),
                &reviews.strip,
            );
        }

        let records = analysis.records(source, &reviews, CandidateThresholds::default());

        assert_eq!(
            vec![json!("candidate"), json!("source")],
            records
                .iter()
                .map(|record| record["type"].clone())
                .collect::<Vec<_>>()
        );
        assert_eq!(json!("ansichtskarte."), records[0]["sentence"]);
        assert_eq!(json!(NOTICE.len() * 5), records[1]["strippedBytes"]);
    }

    #[test]
    fn should_not_count_a_fail_safe_listing_as_stripped() {
        let strip: DescriptionBoilerplate = [sentence(NOTICE)].into_iter().collect();
        let mut analysis = SourceAnalysis::default();
        analysis.add_listing(Some(&listing(NOTICE)), &strip);
        assert_eq!(0, analysis.stripped_bytes);
    }

    #[test]
    fn should_parse_reviewed_candidates_and_skip_source_records() {
        let source = ListingSourceId::new();
        let input = format!(
            "{}\n\n{}\n{}\n",
            json!({"type": "candidate", "listingSourceId": source.to_string(), "sentence": "Free  shipping.", "decision": "STRIP"}),
            json!({"type": "source", "listingSourceId": source.to_string()}),
            json!({"listingSourceId": source.to_string(), "sentence": "Gelaufen.", "decision": "KEEP"}),
        );
        assert_eq!(
            Ok(vec![
                review(
                    source,
                    "free shipping.",
                    DescriptionBoilerplateDecision::Strip
                ),
                review(source, "Gelaufen.", DescriptionBoilerplateDecision::Keep),
            ]),
            parse_reviews(input.as_bytes())
        );
    }

    #[test]
    fn should_reject_review_file_with_an_undecided_candidate() {
        let source = ListingSourceId::new();
        let input = json!({"type": "candidate", "listingSourceId": source.to_string(), "sentence": "Free shipping."}).to_string();
        assert_eq!(
            Err("line 1: `decision` is missing".to_owned()),
            parse_reviews(input.as_bytes())
        );
        let input = json!({"listingSourceId": source.to_string(), "sentence": "Free shipping.", "decision": "strip"}).to_string();
        assert!(parse_reviews(input.as_bytes()).is_err());
    }

    struct DescriptionsFake(Vec<CurrentRawDescription>);

    #[async_trait]
    impl CurrentRawDescriptionReader for DescriptionsFake {
        async fn page(
            &self,
            listing_source_id: Option<ListingSourceId>,
            after: Option<CurrentRawDescriptionCursor>,
            limit: u32,
        ) -> Result<Vec<CurrentRawDescription>, DescriptionBoilerplatePortError> {
            let key = |description: &CurrentRawDescription| {
                (
                    description.listing_source_id,
                    description.product_listing_raw_stream_id,
                )
            };
            let mut rows = self
                .0
                .iter()
                .filter(|row| {
                    listing_source_id.is_none_or(|source| row.listing_source_id == source)
                })
                .filter(|row| {
                    after.is_none_or(|cursor| {
                        key(row)
                            > (
                                cursor.listing_source_id,
                                cursor.product_listing_raw_stream_id,
                            )
                    })
                })
                .cloned()
                .collect::<Vec<_>>();
            rows.sort_by_key(key);
            rows.truncate(limit as usize);
            Ok(rows)
        }
    }

    struct ReviewsFake(Vec<DescriptionBoilerplateReview>);

    #[async_trait]
    impl DescriptionBoilerplateReader for ReviewsFake {
        async fn strip_set(
            &self,
            _listing_source_id: ListingSourceId,
        ) -> Result<DescriptionBoilerplate, DescriptionBoilerplatePortError> {
            unreachable!("analysis reads every review at once")
        }

        async fn reviews(
            &self,
            _listing_source_id: Option<ListingSourceId>,
        ) -> Result<Vec<DescriptionBoilerplateReview>, DescriptionBoilerplatePortError> {
            Ok(self.0.clone())
        }
    }

    #[tokio::test]
    async fn should_analyse_each_source_separately_across_pages() {
        let first = ListingSourceId::new();
        let second = ListingSourceId::new();
        let rows = [
            (first, "A. Shop notice."),
            (first, "B. Shop notice."),
            (second, "C. Shop notice."),
        ]
        .into_iter()
        .map(|(source, text)| CurrentRawDescription {
            listing_source_id: source,
            product_listing_raw_stream_id: ProductListingRawStreamId::new(),
            fragments: Some(listing(text)),
        })
        .collect();
        let mut output = Vec::new();

        let completion = boilerplate_candidates(
            &DescriptionsFake(rows),
            &ReviewsFake(vec![review(
                second,
                "Shop notice.",
                DescriptionBoilerplateDecision::Strip,
            )]),
            None,
            CandidateThresholds {
                min_listings: 2,
                min_share: 0.5,
            },
            1,
            &mut output,
        )
        .await;

        assert_eq!(
            CandidateCompletion {
                sources: 2,
                listings: 3,
                candidates: 1,
                error: None,
            },
            completion
        );
        let records = String::from_utf8(output)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str::<Value>(line).unwrap())
            .collect::<Vec<_>>();
        let summary = |source: ListingSourceId| {
            records
                .iter()
                .find(|record| {
                    record["type"] == "source" && record["listingSourceId"] == source.to_string()
                })
                .unwrap()
                .clone()
        };
        assert_eq!(json!(1), summary(first)["candidates"]);
        assert_eq!(json!(0), summary(second)["candidates"]);
        assert_eq!(
            json!("Shop notice.".len()),
            summary(second)["strippedBytes"]
        );
    }
}

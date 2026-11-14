use crate::ports::ContentNoticeSubmissionFingerprint;
use content_notice_core::content_notice::ContentNotice;
use sha2::{Digest, Sha256};

/// Identifies what a notice submits, so the submission rules can recognise a repeated
/// submission. It is SHA-256 over a canonical encoding of, in this order: the lowercased
/// email address; the target type and identifiers; the submitter role and reason category; the
/// explanation; the additional URLs in their normalized form, sorted; the submitter's name and
/// language.
///
/// Every value is length-prefixed and optional values carry a presence marker, so no two
/// different submissions share an encoding. The values are the validated domain values, so
/// their trimming and URL normalization apply. Changing the encoding changes every
/// fingerprint; stored fingerprints are not recomputed.
pub fn submission_fingerprint(notice: &ContentNotice) -> ContentNoticeSubmissionFingerprint {
    let submitter = notice.submitter();
    let target = notice.target();
    let mut additional_urls = notice
        .additional_urls()
        .as_slice()
        .iter()
        .map(|url| url.as_str())
        .collect::<Vec<_>>();
    additional_urls.sort_unstable();

    let mut encoder = CanonicalEncoder::default();
    encoder.value(&submitter.email.as_str().to_lowercase());
    encoder.value(target.type_code());
    encoder.value(&target.listing_source_id().to_string());
    encoder.optional(
        target
            .product_listing_id()
            .map(|id| id.to_string())
            .as_deref(),
    );
    encoder.value(notice.submitter_role().as_str());
    encoder.value(notice.reason_category().as_str());
    encoder.optional(notice.explanation().map(|explanation| explanation.as_str()));
    encoder.count(additional_urls.len());
    for url in additional_urls {
        encoder.value(url);
    }
    encoder.value(submitter.name.as_str());
    encoder.value(submitter.language.as_str());
    ContentNoticeSubmissionFingerprint::from_sha256(encoder.finish())
}

#[derive(Default)]
struct CanonicalEncoder(Sha256);

impl CanonicalEncoder {
    fn count(&mut self, count: usize) {
        self.0.update((count as u64).to_be_bytes());
    }

    fn value(&mut self, value: &str) {
        self.count(value.len());
        self.0.update(value.as_bytes());
    }

    fn optional(&mut self, value: Option<&str>) {
        match value {
            None => self.0.update([0]),
            Some(value) => {
                self.0.update([1]);
                self.value(value);
            }
        }
    }

    fn finish(self) -> [u8; 32] {
        self.0.finalize().into()
    }
}

#[cfg(test)]
mod tests {
    use super::submission_fingerprint;
    use content_notice_core::{
        additional_url::{AdditionalUrl, AdditionalUrls},
        content_notice::{ContentNotice, NewContentNotice},
        content_notice_id::ContentNoticeId,
        content_notice_target::{ContentNoticeTarget, ListingSourceSnapshot, TargetSnapshot},
        good_faith_statement_version::GoodFaithStatementVersion,
        notice_explanation::NoticeExplanation,
        reason_category::ReasonCategory,
        submitter::{Submitter, SubmitterEmail, SubmitterName},
        submitter_role::SubmitterRole,
    };
    use listing_source_core::{ListingSourceId, ListingSourceName};
    use localization::Language;

    fn input() -> NewContentNotice {
        NewContentNotice {
            id: ContentNoticeId::new(),
            target: ContentNoticeTarget::ListingSource {
                listing_source_id: ListingSourceId::new(),
            },
            target_snapshot: TargetSnapshot {
                listing_source: ListingSourceSnapshot {
                    name: ListingSourceName::try_from("Antik Shop").unwrap(),
                    url: None,
                    partner: false,
                },
                product_listing: None,
            },
            submitter_role: SubmitterRole::RightsHolder,
            reason_category: ReasonCategory::RightsInfringement,
            explanation: Some(NoticeExplanation::try_from("My photograph.").unwrap()),
            additional_urls: urls(&["https://a.example/1", "https://b.example/2"]),
            submitter: Submitter {
                name: SubmitterName::try_from("Jane Doe").unwrap(),
                email: SubmitterEmail::try_from("jane@example.com").unwrap(),
                language: Language::De,
            },
            good_faith_statement_version: GoodFaithStatementVersion::GoodFaithV1,
        }
    }

    fn urls(values: &[&str]) -> AdditionalUrls {
        AdditionalUrls::try_from(
            values
                .iter()
                .map(|value| AdditionalUrl::try_from(*value).unwrap())
                .collect::<Vec<_>>(),
        )
        .unwrap()
    }

    fn fingerprint(input: NewContentNotice) -> [u8; 32] {
        *submission_fingerprint(&ContentNotice::submit(input).unwrap()).as_bytes()
    }

    #[test]
    fn should_give_the_same_fingerprint_for_the_same_submission() {
        let first = input();
        let mut repeated = first.clone();
        repeated.id = ContentNoticeId::new();
        repeated.target_snapshot.listing_source.partner = true;
        repeated.good_faith_statement_version = GoodFaithStatementVersion::GoodFaithV1;

        assert_eq!(fingerprint(first), fingerprint(repeated));
    }

    #[test]
    fn should_ignore_url_order_email_case_and_what_normalization_removes() {
        let first = input();
        let mut normalized_alike = first.clone();
        normalized_alike.additional_urls =
            urls(&["HTTPS://B.example:443/2", "https://a.example/1"]);
        normalized_alike.submitter.email = SubmitterEmail::try_from("Jane@Example.COM").unwrap();
        normalized_alike.explanation =
            Some(NoticeExplanation::try_from("  My photograph.\n").unwrap());

        assert_eq!(fingerprint(first), fingerprint(normalized_alike));
    }

    #[test]
    fn should_change_when_any_covered_value_changes() {
        let base = input();
        let base_fingerprint = fingerprint(base.clone());
        let mut variants = Vec::new();
        let mut other_target = base.clone();
        other_target.target = ContentNoticeTarget::ListingSource {
            listing_source_id: ListingSourceId::new(),
        };
        variants.push(other_target);
        let mut other_role = base.clone();
        other_role.submitter_role = SubmitterRole::IllegalContentReporter;
        variants.push(other_role);
        let mut other_category = base.clone();
        other_category.reason_category = ReasonCategory::Other;
        variants.push(other_category);
        let mut other_explanation = base.clone();
        other_explanation.explanation = Some(NoticeExplanation::try_from("Other.").unwrap());
        variants.push(other_explanation);
        let mut fewer_urls = base.clone();
        fewer_urls.additional_urls = urls(&["https://a.example/1"]);
        variants.push(fewer_urls);
        let mut other_name = base.clone();
        other_name.submitter.name = SubmitterName::try_from("John Doe").unwrap();
        variants.push(other_name);
        let mut other_language = base.clone();
        other_language.submitter.language = Language::En;
        variants.push(other_language);
        let mut other_email = base;
        other_email.submitter.email = SubmitterEmail::try_from("john@example.com").unwrap();
        variants.push(other_email);

        for variant in variants {
            assert_ne!(base_fingerprint, fingerprint(variant));
        }
    }

    #[test]
    fn should_keep_value_boundaries_apart() {
        let two_urls = input();
        let mut one_concatenated_url = two_urls.clone();
        one_concatenated_url.additional_urls = urls(&["https://a.example/1https://b.example/2"]);

        assert_ne!(
            fingerprint(two_urls.clone()),
            fingerprint(one_concatenated_url)
        );

        let mut provider_opt_out = two_urls;
        provider_opt_out.submitter_role = SubmitterRole::Provider;
        provider_opt_out.reason_category = ReasonCategory::ProviderOptOut;
        let mut without_explanation = provider_opt_out.clone();
        without_explanation.explanation = None;

        assert_ne!(
            fingerprint(provider_opt_out),
            fingerprint(without_explanation)
        );
    }
}

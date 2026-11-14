use crate::{
    additional_url::AdditionalUrls,
    content_notice_id::ContentNoticeId,
    content_notice_state::ContentNoticeState,
    content_notice_target::{ContentNoticeTarget, TargetSnapshot},
    good_faith_statement_version::GoodFaithStatementVersion,
    notice_explanation::NoticeExplanation,
    reason_category::ReasonCategory,
    submitter::Submitter,
    submitter_role::SubmitterRole,
};

/// A notice that a listing or a listing source should be reviewed, for example a provider's
/// opt-out or a report of a rights infringement. See `docs/content-notices.md`.
#[derive(Debug, Clone, PartialEq)]
pub struct ContentNotice {
    id: ContentNoticeId,
    target: ContentNoticeTarget,
    target_snapshot: TargetSnapshot,
    submitter_role: SubmitterRole,
    reason_category: ReasonCategory,
    explanation: Option<NoticeExplanation>,
    additional_urls: AdditionalUrls,
    submitter: Submitter,
    good_faith_statement_version: GoodFaithStatementVersion,
    state: ContentNoticeState,
}

/// The submitted content of a new notice. The good-faith statement is confirmed by naming
/// the version of its wording; a notice without a confirmed statement cannot be expressed.
#[derive(Debug, Clone, PartialEq)]
pub struct NewContentNotice {
    pub id: ContentNoticeId,
    pub target: ContentNoticeTarget,
    pub target_snapshot: TargetSnapshot,
    pub submitter_role: SubmitterRole,
    pub reason_category: ReasonCategory,
    pub explanation: Option<NoticeExplanation>,
    pub additional_urls: AdditionalUrls,
    pub submitter: Submitter,
    pub good_faith_statement_version: GoodFaithStatementVersion,
}

#[doc(hidden)]
#[derive(Debug, Clone, PartialEq)]
pub struct RehydratedContentNoticeState {
    pub id: ContentNoticeId,
    pub target: ContentNoticeTarget,
    pub target_snapshot: TargetSnapshot,
    pub submitter_role: SubmitterRole,
    pub reason_category: ReasonCategory,
    pub explanation: Option<NoticeExplanation>,
    pub additional_urls: AdditionalUrls,
    pub submitter: Submitter,
    pub good_faith_statement_version: GoodFaithStatementVersion,
    pub state: ContentNoticeState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ContentNoticeInvariantError {
    #[error("reason category {} requires an explanation", reason_category.as_str())]
    ExplanationRequired { reason_category: ReasonCategory },
    #[error(
        "reason category PROVIDER_OPT_OUT requires submitter role PROVIDER, not {}",
        submitter_role.as_str()
    )]
    ProviderOptOutRequiresProvider { submitter_role: SubmitterRole },
    #[error("target snapshot does not describe the kind of target the notice is about")]
    TargetSnapshotMismatch,
}

impl ContentNotice {
    pub fn submit(input: NewContentNotice) -> Result<Self, ContentNoticeInvariantError> {
        Self::rehydrate(RehydratedContentNoticeState {
            id: input.id,
            target: input.target,
            target_snapshot: input.target_snapshot,
            submitter_role: input.submitter_role,
            reason_category: input.reason_category,
            explanation: input.explanation,
            additional_urls: input.additional_urls,
            submitter: input.submitter,
            good_faith_statement_version: input.good_faith_statement_version,
            state: ContentNoticeState::Submitted,
        })
    }

    /// Rebuilds a stored notice and fails closed when the stored state breaks an invariant.
    #[doc(hidden)]
    pub fn rehydrate(
        state: RehydratedContentNoticeState,
    ) -> Result<Self, ContentNoticeInvariantError> {
        if !state.target_snapshot.matches(state.target) {
            return Err(ContentNoticeInvariantError::TargetSnapshotMismatch);
        }
        Self::check_reason(
            state.submitter_role,
            state.reason_category,
            state.explanation.as_ref(),
        )?;
        Ok(Self {
            id: state.id,
            target: state.target,
            target_snapshot: state.target_snapshot,
            submitter_role: state.submitter_role,
            reason_category: state.reason_category,
            explanation: state.explanation,
            additional_urls: state.additional_urls,
            submitter: state.submitter,
            good_faith_statement_version: state.good_faith_statement_version,
            state: state.state,
        })
    }

    /// Checks the rules that tie the reason to the submitter role and the explanation. They do
    /// not depend on the target, so a submission can be checked before its target is resolved.
    pub fn check_reason(
        submitter_role: SubmitterRole,
        reason_category: ReasonCategory,
        explanation: Option<&NoticeExplanation>,
    ) -> Result<(), ContentNoticeInvariantError> {
        if reason_category == ReasonCategory::ProviderOptOut
            && submitter_role != SubmitterRole::Provider
        {
            return Err(
                ContentNoticeInvariantError::ProviderOptOutRequiresProvider { submitter_role },
            );
        }
        if explanation.is_none() && reason_category.requires_explanation() {
            return Err(ContentNoticeInvariantError::ExplanationRequired { reason_category });
        }
        Ok(())
    }

    pub fn id(&self) -> ContentNoticeId {
        self.id
    }

    pub fn target(&self) -> ContentNoticeTarget {
        self.target
    }

    pub fn target_snapshot(&self) -> &TargetSnapshot {
        &self.target_snapshot
    }

    pub fn submitter_role(&self) -> SubmitterRole {
        self.submitter_role
    }

    pub fn reason_category(&self) -> ReasonCategory {
        self.reason_category
    }

    pub fn explanation(&self) -> Option<&NoticeExplanation> {
        self.explanation.as_ref()
    }

    pub fn additional_urls(&self) -> &AdditionalUrls {
        &self.additional_urls
    }

    pub fn submitter(&self) -> &Submitter {
        &self.submitter
    }

    pub fn good_faith_statement_version(&self) -> GoodFaithStatementVersion {
        self.good_faith_statement_version
    }

    pub fn state(&self) -> ContentNoticeState {
        self.state
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        content_notice_target::{ListingSourceSnapshot, ProductListingSnapshot},
        submitter::{SubmitterEmail, SubmitterName},
    };
    use listing_source_core::{ListingSourceId, ListingSourceName};
    use localization::Language;
    use product_listing_core::product_listing_id::ProductListingId;
    use strum::IntoEnumIterator;
    use url::Url;

    fn listing_source_notice(
        submitter_role: SubmitterRole,
        reason_category: ReasonCategory,
        explanation: Option<&str>,
    ) -> NewContentNotice {
        NewContentNotice {
            id: ContentNoticeId::new(),
            target: ContentNoticeTarget::ListingSource {
                listing_source_id: ListingSourceId::new(),
            },
            target_snapshot: TargetSnapshot {
                listing_source: ListingSourceSnapshot {
                    name: ListingSourceName::try_from("Antik Shop").unwrap(),
                    url: Some(Url::parse("https://shop.example/").unwrap()),
                    partner: false,
                },
                product_listing: None,
            },
            submitter_role,
            reason_category,
            explanation: explanation.map(|value| NoticeExplanation::try_from(value).unwrap()),
            additional_urls: AdditionalUrls::default(),
            submitter: Submitter {
                name: SubmitterName::try_from("Jane Doe").unwrap(),
                email: SubmitterEmail::try_from("jane@shop.example").unwrap(),
                language: Language::De,
            },
            good_faith_statement_version: GoodFaithStatementVersion::GoodFaithV1,
        }
    }

    fn product_listing_snapshot() -> ProductListingSnapshot {
        ProductListingSnapshot {
            title: None,
            url: Url::parse("https://shop.example/item/1").unwrap(),
        }
    }

    #[test]
    fn should_start_new_notice_in_submitted_state() {
        let input = listing_source_notice(
            SubmitterRole::RightsHolder,
            ReasonCategory::RightsInfringement,
            Some("My photograph is used without licence."),
        );

        let notice = ContentNotice::submit(input.clone()).unwrap();

        assert_eq!(ContentNoticeState::Submitted, notice.state());
        assert_eq!(input.id, notice.id());
        assert_eq!(input.target, notice.target());
        assert_eq!(
            GoodFaithStatementVersion::GoodFaithV1,
            notice.good_faith_statement_version()
        );
    }

    #[test]
    fn should_allow_provider_opt_out_without_explanation() {
        let notice = ContentNotice::submit(listing_source_notice(
            SubmitterRole::Provider,
            ReasonCategory::ProviderOptOut,
            None,
        ))
        .unwrap();

        assert_eq!(None, notice.explanation());
    }

    #[test]
    fn should_require_explanation_for_every_other_category() {
        for reason_category in
            ReasonCategory::iter().filter(|category| *category != ReasonCategory::ProviderOptOut)
        {
            assert_eq!(
                Err(ContentNoticeInvariantError::ExplanationRequired { reason_category }),
                ContentNotice::submit(listing_source_notice(
                    SubmitterRole::Provider,
                    reason_category,
                    None,
                ))
            );
            assert!(
                ContentNotice::submit(listing_source_notice(
                    SubmitterRole::Provider,
                    reason_category,
                    Some("Reasoned explanation."),
                ))
                .is_ok()
            );
        }
    }

    #[test]
    fn should_allow_provider_opt_out_only_for_provider_role() {
        for submitter_role in SubmitterRole::iter().filter(|role| *role != SubmitterRole::Provider)
        {
            assert_eq!(
                Err(ContentNoticeInvariantError::ProviderOptOutRequiresProvider { submitter_role }),
                ContentNotice::submit(listing_source_notice(
                    submitter_role,
                    ReasonCategory::ProviderOptOut,
                    Some("Please remove us."),
                ))
            );
        }
    }

    #[test]
    fn should_accept_product_listing_target_with_product_snapshot() {
        let mut input = listing_source_notice(
            SubmitterRole::IllegalContentReporter,
            ReasonCategory::IllegalContent,
            Some("Prohibited symbol."),
        );
        input.target = ContentNoticeTarget::ProductListing {
            product_listing_id: ProductListingId::new(),
            listing_source_id: ListingSourceId::new(),
        };
        input.target_snapshot.product_listing = Some(product_listing_snapshot());

        assert!(ContentNotice::submit(input).is_ok());
    }

    #[test]
    fn should_reject_snapshot_of_other_target_kind() {
        let mut listing_target_without_product = listing_source_notice(
            SubmitterRole::Provider,
            ReasonCategory::ProviderOptOut,
            None,
        );
        listing_target_without_product.target = ContentNoticeTarget::ProductListing {
            product_listing_id: ProductListingId::new(),
            listing_source_id: ListingSourceId::new(),
        };
        let mut source_target_with_product = listing_source_notice(
            SubmitterRole::Provider,
            ReasonCategory::ProviderOptOut,
            None,
        );
        source_target_with_product.target_snapshot.product_listing =
            Some(product_listing_snapshot());

        for input in [listing_target_without_product, source_target_with_product] {
            assert_eq!(
                Err(ContentNoticeInvariantError::TargetSnapshotMismatch),
                ContentNotice::submit(input)
            );
        }
    }

    #[test]
    fn should_fail_closed_when_rehydrating_state_that_breaks_an_invariant() {
        let input = listing_source_notice(SubmitterRole::RightsHolder, ReasonCategory::Other, None);

        let result = ContentNotice::rehydrate(RehydratedContentNoticeState {
            id: input.id,
            target: input.target,
            target_snapshot: input.target_snapshot,
            submitter_role: input.submitter_role,
            reason_category: input.reason_category,
            explanation: input.explanation,
            additional_urls: input.additional_urls,
            submitter: input.submitter,
            good_faith_statement_version: input.good_faith_statement_version,
            state: ContentNoticeState::Submitted,
        });

        assert_eq!(
            Err(ContentNoticeInvariantError::ExplanationRequired {
                reason_category: ReasonCategory::Other,
            }),
            result
        );
    }
}

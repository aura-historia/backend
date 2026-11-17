//! Transactional email to an address that does not belong to a user account.
//!
//! A producer in any bounded context enqueues an external email in its own business
//! transaction and the notification delivery pipeline sends it; `docs/events/flow.md`
//! (External email delivery) owns the contract.

use content_notice_core::{
    content_notice::ContentNotice,
    content_notice_id::ContentNoticeId,
    content_notice_target::{ContentNoticeTarget, ProductListingTitleSnapshot},
    reason_category::ReasonCategory,
};
use listing_source_core::ListingSourceName;
use serde_email::Email;
use strum::IntoEnumIterator;
use time::OffsetDateTime;
use uuid::Uuid;

/// The address an external email is sent to: a bare ASCII address without display text, as
/// the email provider requires; a producer converts an international domain to punycode.
/// It is personal data, so `Debug` and errors never repeat it.
#[derive(Clone, PartialEq, Eq)]
pub struct ExternalEmailRecipient(Email);

/// Why a recipient address was rejected. It never repeats the address.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum InvalidExternalEmailRecipient {
    #[error("email address is invalid")]
    Invalid,
    #[error("email address must be a bare address without display text, spaces or controls")]
    NotBareAddress,
    #[error("email address must be ASCII, with an international domain in punycode")]
    NotAscii,
    #[error("email address must not exceed {max_bytes} bytes")]
    TooLong { max_bytes: usize },
}

impl ExternalEmailRecipient {
    pub const MAX_BYTES: usize = 320;

    pub fn as_email(&self) -> &Email {
        &self.0
    }

    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }
}

impl TryFrom<&str> for ExternalEmailRecipient {
    type Error = InvalidExternalEmailRecipient;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        let value = value.trim();
        if value.len() > Self::MAX_BYTES {
            return Err(InvalidExternalEmailRecipient::TooLong {
                max_bytes: Self::MAX_BYTES,
            });
        }
        if value.chars().any(|character| {
            matches!(character, '<' | '>') || character.is_whitespace() || character.is_control()
        }) {
            return Err(InvalidExternalEmailRecipient::NotBareAddress);
        }
        if !value.is_ascii() {
            return Err(InvalidExternalEmailRecipient::NotAscii);
        }
        // The email error repeats the address, so it is not kept as the cause.
        Email::new(value)
            .map(Self)
            .map_err(|_| InvalidExternalEmailRecipient::Invalid)
    }
}

impl TryFrom<String> for ExternalEmailRecipient {
    type Error = InvalidExternalEmailRecipient;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::try_from(value.as_str())
    }
}

impl std::fmt::Debug for ExternalEmailRecipient {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_tuple("ExternalEmailRecipient")
            .field(&"<redacted>")
            .finish()
    }
}

/// The typed template variables of an external email. Each variant is one template kind.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExternalEmailContent {
    /// Confirms to a notifier that their content notice was received (DSA Art. 16(4)).
    ContentNoticeReceipt(ContentNoticeReceipt),
}

impl ExternalEmailContent {
    pub const fn template(&self) -> ExternalEmailTemplate {
        match self {
            Self::ContentNoticeReceipt(_) => ExternalEmailTemplate::ContentNoticeReceipt,
        }
    }
}

/// The template kind of an external email, one per [`ExternalEmailContent`] variant.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, strum_macros::EnumIter)]
pub enum ExternalEmailTemplate {
    ContentNoticeReceipt,
}

impl ExternalEmailTemplate {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ContentNoticeReceipt => "CONTENT_NOTICE_RECEIPT",
        }
    }

    pub fn from_code(code: &str) -> Option<Self> {
        Self::iter().find(|template| template.as_str() == code)
    }
}

/// The business record that caused an external email. One origin enqueues each template
/// kind at most once, and erasing an origin erases its messages.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ExternalEmailOrigin {
    ContentNotice(ContentNoticeId),
}

impl ExternalEmailOrigin {
    pub const fn origin_type(self) -> &'static str {
        match self {
            Self::ContentNotice(_) => "CONTENT_NOTICE",
        }
    }

    pub const fn origin_id(self) -> Uuid {
        match self {
            Self::ContentNotice(id) => id.into_uuid(),
        }
    }
}

/// Variables of a content notice receipt. They are data from Aura Historia's side only:
/// nothing the notifier typed is echoed back, so the submission endpoint cannot send
/// arbitrary content to the address it was given.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContentNoticeReceipt {
    pub content_notice_id: ContentNoticeId,
    /// When the notice was stored.
    pub received_at: OffsetDateTime,
    /// The listing source's name from the notice's target snapshot.
    pub provider_name: ListingSourceName,
    pub target: ContentNoticeReceiptTarget,
    pub reason_category: ReasonCategory,
}

/// What the notice is about, as the receipt names it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ContentNoticeReceiptTarget {
    /// The whole provider, with all of its listings.
    ListingSource,
    /// One listing, with its title from the snapshot when it had one.
    ProductListing {
        title: Option<ProductListingTitleSnapshot>,
    },
}

impl ContentNoticeReceipt {
    /// The receipt of a stored notice: its ID, the target snapshot's names and the category.
    /// The submitter's name, email, explanation and URLs are never taken.
    pub fn for_notice(notice: &ContentNotice, received_at: OffsetDateTime) -> Self {
        let snapshot = notice.target_snapshot();
        Self {
            content_notice_id: notice.id(),
            received_at,
            provider_name: snapshot.listing_source.name.clone(),
            target: match notice.target() {
                ContentNoticeTarget::ListingSource { .. } => {
                    ContentNoticeReceiptTarget::ListingSource
                }
                ContentNoticeTarget::ProductListing { .. } => {
                    ContentNoticeReceiptTarget::ProductListing {
                        title: snapshot
                            .product_listing
                            .as_ref()
                            .and_then(|listing| listing.title.clone()),
                    }
                }
            },
            reason_category: notice.reason_category(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn should_accept_a_trimmed_bare_recipient_address() -> Result<(), InvalidExternalEmailRecipient>
    {
        let recipient = ExternalEmailRecipient::try_from(" notifier@example.test ")?;

        assert_eq!("notifier@example.test", recipient.as_str());
        assert_eq!(
            "ExternalEmailRecipient(\"<redacted>\")",
            format!("{recipient:?}")
        );
        Ok(())
    }

    #[test]
    fn should_reject_recipients_that_are_not_bare_addresses_without_repeating_them() {
        let too_long = format!("{}@example.test", "a".repeat(320));
        for (value, expected) in [
            (
                "Jane <jane@example.test>",
                InvalidExternalEmailRecipient::NotBareAddress,
            ),
            (
                "\"jane doe\"@example.test",
                InvalidExternalEmailRecipient::NotBareAddress,
            ),
            ("not-an-address", InvalidExternalEmailRecipient::Invalid),
            (
                "jane@müller.example.test",
                InvalidExternalEmailRecipient::NotAscii,
            ),
            (
                "dörte@example.test",
                InvalidExternalEmailRecipient::NotAscii,
            ),
            (
                too_long.as_str(),
                InvalidExternalEmailRecipient::TooLong { max_bytes: 320 },
            ),
        ] {
            let error =
                ExternalEmailRecipient::try_from(value).expect_err("recipient should be rejected");
            assert_eq!(expected, error);
            assert!(!error.to_string().contains("example.test"));
        }
    }

    #[test]
    fn should_round_trip_every_template_code() {
        let templates = ExternalEmailTemplate::iter().collect::<Vec<_>>();
        let codes = templates
            .iter()
            .map(|template| template.as_str())
            .collect::<HashSet<_>>();

        assert_eq!(templates.len(), codes.len());
        for template in templates {
            assert_eq!(
                Some(template),
                ExternalEmailTemplate::from_code(template.as_str())
            );
        }
        assert_eq!(
            None,
            ExternalEmailTemplate::from_code("content_notice_receipt")
        );
    }

    #[test]
    fn should_identify_a_content_notice_origin_by_the_notice_id() {
        let id = ContentNoticeId::new();
        let origin = ExternalEmailOrigin::ContentNotice(id);

        assert_eq!("CONTENT_NOTICE", origin.origin_type());
        assert_eq!(id.into_uuid(), origin.origin_id());
    }

    /// A notice about a listing with `listing`'s title, or about the whole provider when
    /// `listing` is `None`.
    fn notice(listing: Option<Option<&str>>) -> Result<ContentNotice, Box<dyn std::error::Error>> {
        use content_notice_core::{
            additional_url::{AdditionalUrl, AdditionalUrls},
            content_notice::NewContentNotice,
            content_notice_target::{
                ContentNoticeTarget, ListingSourceSnapshot, ProductListingSnapshot, TargetSnapshot,
            },
            good_faith_statement_version::GoodFaithStatementVersion,
            notice_explanation::NoticeExplanation,
            submitter::{Submitter, SubmitterEmail, SubmitterName},
            submitter_role::SubmitterRole,
        };
        use listing_source_core::ListingSourceId;
        use product_listing_core::product_listing_id::ProductListingId;

        let listing_source_id = ListingSourceId::new();
        let (target, product_listing) = match listing {
            Some(title) => (
                ContentNoticeTarget::ProductListing {
                    product_listing_id: ProductListingId::new(),
                    listing_source_id,
                },
                Some(ProductListingSnapshot {
                    title: title
                        .map(ProductListingTitleSnapshot::try_from)
                        .transpose()?,
                    url: url::Url::parse("https://shop.example/vase")?,
                }),
            ),
            None => (
                ContentNoticeTarget::ListingSource { listing_source_id },
                None,
            ),
        };
        Ok(ContentNotice::submit(NewContentNotice {
            id: ContentNoticeId::new(),
            target,
            target_snapshot: TargetSnapshot {
                listing_source: ListingSourceSnapshot {
                    name: ListingSourceName::try_from("Antik Shop")?,
                    url: None,
                    partner: false,
                },
                product_listing,
            },
            submitter_role: SubmitterRole::RightsHolder,
            reason_category: ReasonCategory::RightsInfringement,
            explanation: Some(NoticeExplanation::try_from("submitter explanation")?),
            additional_urls: AdditionalUrls::try_from(vec![AdditionalUrl::try_from(
                "https://submitter.example/",
            )?])?,
            submitter: Submitter {
                name: Some(SubmitterName::try_from("Submitter Name")?),
                email: Some(SubmitterEmail::try_from("submitter@example.test")?),
                language: localization::Language::De,
            },
            good_faith_statement_version: GoodFaithStatementVersion::GoodFaithV1,
        })?)
    }

    #[test]
    fn should_take_only_our_own_data_from_a_listing_notice_into_its_receipt()
    -> Result<(), Box<dyn std::error::Error>> {
        let notice = notice(Some(Some("Jugendstil-Vase")))?;
        let received_at = OffsetDateTime::UNIX_EPOCH;

        let receipt = ContentNoticeReceipt::for_notice(&notice, received_at);

        assert_eq!(
            ContentNoticeReceipt {
                content_notice_id: notice.id(),
                received_at,
                provider_name: ListingSourceName::try_from("Antik Shop")?,
                target: ContentNoticeReceiptTarget::ProductListing {
                    title: Some(ProductListingTitleSnapshot::try_from("Jugendstil-Vase")?),
                },
                reason_category: ReasonCategory::RightsInfringement,
            },
            receipt
        );
        let content = ExternalEmailContent::ContentNoticeReceipt(receipt);
        assert_eq!(
            ExternalEmailTemplate::ContentNoticeReceipt,
            content.template()
        );
        assert_eq!("CONTENT_NOTICE_RECEIPT", content.template().as_str());
        Ok(())
    }

    #[test]
    fn should_name_the_whole_provider_in_the_receipt_of_a_listing_source_notice()
    -> Result<(), Box<dyn std::error::Error>> {
        let receipt = ContentNoticeReceipt::for_notice(&notice(None)?, OffsetDateTime::UNIX_EPOCH);

        assert_eq!(ContentNoticeReceiptTarget::ListingSource, receipt.target);
        Ok(())
    }

    #[test]
    fn should_keep_a_listing_without_a_title_a_listing_in_its_receipt()
    -> Result<(), Box<dyn std::error::Error>> {
        let receipt =
            ContentNoticeReceipt::for_notice(&notice(Some(None))?, OffsetDateTime::UNIX_EPOCH);

        assert_eq!(
            ContentNoticeReceiptTarget::ProductListing { title: None },
            receipt.target
        );
        Ok(())
    }
}

use crate::bounded_text::bounded_text_newtype;
use listing_source_core::{ListingSourceId, ListingSourceName};
use product_listing_core::product_listing_id::ProductListingId;
use url::Url;

/// What a content notice is about.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ContentNoticeTarget {
    /// The provider (shop) as a whole.
    ListingSource { listing_source_id: ListingSourceId },
    /// One listing. Its listing source is kept so that per-source queries cover both kinds.
    ProductListing {
        product_listing_id: ProductListingId,
        listing_source_id: ListingSourceId,
    },
}

impl ContentNoticeTarget {
    pub fn target_type(self) -> ContentNoticeTargetType {
        match self {
            Self::ListingSource { .. } => ContentNoticeTargetType::ListingSource,
            Self::ProductListing { .. } => ContentNoticeTargetType::ProductListing,
        }
    }

    pub fn listing_source_id(self) -> ListingSourceId {
        match self {
            Self::ListingSource { listing_source_id }
            | Self::ProductListing {
                listing_source_id, ..
            } => listing_source_id,
        }
    }

    pub fn product_listing_id(self) -> Option<ProductListingId> {
        match self {
            Self::ListingSource { .. } => None,
            Self::ProductListing {
                product_listing_id, ..
            } => Some(product_listing_id),
        }
    }
}

/// The kind of thing a content notice is about.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, strum_macros::EnumIter)]
pub enum ContentNoticeTargetType {
    ListingSource,
    ProductListing,
}

impl ContentNoticeTargetType {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ListingSource => "LISTING_SOURCE",
            Self::ProductListing => "PRODUCT_LISTING",
        }
    }

    pub fn from_code(value: &str) -> Option<Self> {
        use strum::IntoEnumIterator;

        Self::iter().find(|target_type| target_type.as_str() == value)
    }
}

/// The target as it was when the notice was submitted. A notice is evidence, so the snapshot
/// stays as it was even when the target later changes or disappears.
#[derive(Debug, Clone, PartialEq)]
pub struct TargetSnapshot {
    pub listing_source: ListingSourceSnapshot,
    /// Present exactly when the target is a product listing.
    pub product_listing: Option<ProductListingSnapshot>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ListingSourceSnapshot {
    pub name: ListingSourceName,
    pub url: Option<Url>,
    /// Whether the listing source was a partner's at submission.
    pub partner: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ProductListingSnapshot {
    pub title: Option<ProductListingTitleSnapshot>,
    pub url: Url,
}

bounded_text_newtype!(
    /// A listing's display title at submission: trimmed, 1–500 characters, no control
    /// characters.
    ProductListingTitleSnapshot,
    max_chars(500),
    allowed_controls(&[])
);

impl TargetSnapshot {
    /// Whether this snapshot describes the same kind of target as `target`.
    pub fn matches(&self, target: ContentNoticeTarget) -> bool {
        self.product_listing.is_some() == target.product_listing_id().is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bounded_text::BoundedTextError;

    fn listing_source_snapshot() -> ListingSourceSnapshot {
        ListingSourceSnapshot {
            name: ListingSourceName::try_from("Antik Shop").unwrap(),
            url: None,
            partner: false,
        }
    }

    fn product_listing_snapshot() -> ProductListingSnapshot {
        ProductListingSnapshot {
            title: None,
            url: Url::parse("https://shop.example/item/1").unwrap(),
        }
    }

    #[test]
    fn should_expose_listing_source_of_both_target_kinds() {
        let listing_source_id = ListingSourceId::new();
        let product_listing_id = ProductListingId::new();

        let source = ContentNoticeTarget::ListingSource { listing_source_id };
        let listing = ContentNoticeTarget::ProductListing {
            product_listing_id,
            listing_source_id,
        };

        assert_eq!(ContentNoticeTargetType::ListingSource, source.target_type());
        assert_eq!(
            ContentNoticeTargetType::ProductListing,
            listing.target_type()
        );
        assert_eq!(listing_source_id, source.listing_source_id());
        assert_eq!(None, source.product_listing_id());
        assert_eq!(listing_source_id, listing.listing_source_id());
        assert_eq!(Some(product_listing_id), listing.product_listing_id());
    }

    #[test]
    fn should_round_trip_every_target_type_code() {
        use strum::IntoEnumIterator;

        for target_type in ContentNoticeTargetType::iter() {
            assert_eq!(
                Some(target_type),
                ContentNoticeTargetType::from_code(target_type.as_str())
            );
        }
        assert_eq!(None, ContentNoticeTargetType::from_code("listing_source"));
    }

    #[test]
    fn should_match_snapshot_only_to_target_of_same_kind() {
        let source = ContentNoticeTarget::ListingSource {
            listing_source_id: ListingSourceId::new(),
        };
        let listing = ContentNoticeTarget::ProductListing {
            product_listing_id: ProductListingId::new(),
            listing_source_id: ListingSourceId::new(),
        };
        let source_snapshot = TargetSnapshot {
            listing_source: listing_source_snapshot(),
            product_listing: None,
        };
        let listing_snapshot = TargetSnapshot {
            listing_source: listing_source_snapshot(),
            product_listing: Some(product_listing_snapshot()),
        };

        assert!(source_snapshot.matches(source));
        assert!(listing_snapshot.matches(listing));
        assert!(!source_snapshot.matches(listing));
        assert!(!listing_snapshot.matches(source));
    }

    #[test]
    fn should_bound_product_listing_title_snapshot() {
        let longest = "t".repeat(ProductListingTitleSnapshot::MAX_CHARS);
        assert!(ProductListingTitleSnapshot::try_from(longest.as_str()).is_ok());
        assert_eq!(
            Err(BoundedTextError::TooLong {
                max_chars: 500,
                actual_chars: 501,
            }),
            ProductListingTitleSnapshot::try_from("t".repeat(501))
        );
        assert_eq!(
            Err(BoundedTextError::Blank),
            ProductListingTitleSnapshot::try_from(" ")
        );
        assert_eq!(
            Err(BoundedTextError::ControlCharacter),
            ProductListingTitleSnapshot::try_from("Vase\nRot")
        );
    }
}

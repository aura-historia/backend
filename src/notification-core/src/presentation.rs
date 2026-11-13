use crate::notification::NotificationContent;
use listing_source_core::ListingSourceId;
use product_listing_core::content_policy::{
    ContentPolicyDecision, may_show_product_listing_images,
};
use std::collections::HashSet;
use url::Url;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NotificationImagePresentation {
    pub url: Option<Url>,
    pub content_policy: Option<ContentPolicyDecision>,
}

pub fn present_image(
    image: Option<Url>,
    content_policy: Option<ContentPolicyDecision>,
    show_unassessed_or_sensitive_content: bool,
) -> Option<NotificationImagePresentation> {
    image.map(|url| NotificationImagePresentation {
        url: may_show_product_listing_images(content_policy, show_unassessed_or_sensitive_content)
            .then_some(url),
        content_policy,
    })
}

/// ListingSources whose operator holds an active Partnership when notifications are
/// presented. Resolved when they are read or sent and never stored with them, so a
/// partner tag ends with the Partnership.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PartnerListingSources(HashSet<ListingSourceId>);

impl PartnerListingSources {
    pub fn contains(&self, listing_source_id: ListingSourceId) -> bool {
        self.0.contains(&listing_source_id)
    }

    /// Whether `content` names a listing of a partner's ListingSource.
    pub fn names_partner(&self, content: &NotificationContent) -> bool {
        content
            .listing_source_id()
            .is_some_and(|listing_source_id| self.contains(listing_source_id))
    }
}

impl FromIterator<ListingSourceId> for PartnerListingSources {
    fn from_iter<I: IntoIterator<Item = ListingSourceId>>(iter: I) -> Self {
        Self(iter.into_iter().collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use product_listing_core::content_policy::SensitiveContentCategory;
    use rstest::rstest;

    #[rstest]
    #[case(None, false, false)]
    #[case(Some(ContentPolicyDecision::Allowed), false, true)]
    #[case(
        Some(ContentPolicyDecision::RequiresConsent(SensitiveContentCategory::NaziGermany)),
        false,
        false
    )]
    #[case(None, true, true)]
    fn should_apply_listing_content_policy_to_notification_image(
        #[case] decision: Option<ContentPolicyDecision>,
        #[case] preference: bool,
        #[case] visible: bool,
    ) -> Result<(), url::ParseError> {
        let image = Url::parse("https://shop.example/image.jpg")?;
        let presentation = present_image(Some(image), decision, preference);
        assert_eq!(
            presentation.is_some_and(|image| image.url.is_some()),
            visible
        );
        Ok(())
    }
}

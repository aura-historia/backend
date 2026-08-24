use product_listing_core::{product_image::ProductImage, prohibited_content::ProhibitedContent};
use url::Url;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NotificationImagePresentation {
    pub url: Option<Url>,
    pub prohibited_content: ProhibitedContent,
}

pub fn present_image(
    image: Option<ProductImage>,
    prohibited_content_consent: bool,
) -> Option<NotificationImagePresentation> {
    image.map(|image| NotificationImagePresentation {
        url: (image.prohibited_content.is_safe() || prohibited_content_consent)
            .then_some(image.url),
        prohibited_content: image.prohibited_content,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::rstest;

    #[rstest]
    #[case(ProhibitedContent::None, false, true)]
    #[case(ProhibitedContent::Unknown, false, false)]
    #[case(ProhibitedContent::Unknown, true, true)]
    fn should_filter_notification_image_url_by_consent(
        #[case] prohibited_content: ProhibitedContent,
        #[case] consent: bool,
        #[case] includes_url: bool,
    ) -> Result<(), url::ParseError> {
        let image = ProductImage {
            url: Url::parse("https://shop.example/image.jpg")?,
            prohibited_content,
        };

        let presentation = present_image(Some(image), consent);

        assert_eq!(
            includes_url,
            presentation
                .as_ref()
                .is_some_and(|image| image.url.is_some())
        );
        assert_eq!(
            Some(prohibited_content),
            presentation.map(|image| image.prohibited_content)
        );
        Ok(())
    }

    #[test]
    fn should_keep_no_notification_image_as_none() {
        assert_eq!(None, present_image(None, false));
    }
}

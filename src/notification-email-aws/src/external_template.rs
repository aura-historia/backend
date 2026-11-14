//! Template selection and variables of external emails: transactional email to an address
//! that does not belong to a user account (`docs/events/flow.md`, External email delivery).

use crate::template_mapping::{EmailTemplateType, FrontendLinks};
use notification_core::external_email::ExternalEmailContent;
use notification_service::ports::notification_delivery_repository::ExternalEmailDeliverySource;
use serde_json::{Value, json};

pub(crate) const fn external_template_type(content: &ExternalEmailContent) -> EmailTemplateType {
    match content {
        ExternalEmailContent::TestProbe(_) => EmailTemplateType::ExternalEmailTestProbe,
    }
}

/// Template variables of an external email. The recipient has no account, so the data
/// carries no name and no account or notification-settings links.
pub(crate) fn external_template_data(
    source: &ExternalEmailDeliverySource,
    links: FrontendLinks<'_>,
) -> Value {
    let mut data = match &source.content {
        ExternalEmailContent::TestProbe(probe) => json!({ "reference": probe.reference() }),
    };
    data["home_url"] = json!(links.home());
    data
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::template_mapping::{EmailLanguage, s3_template_key};
    use localization::Language;
    use notification_core::{
        external_email::{ExternalEmailRecipient, ExternalEmailTestProbe},
        external_email_message_id::ExternalEmailMessageId,
        notification_delivery_id::NotificationDeliveryId,
    };
    use rstest::rstest;

    fn external_source(language: Language) -> ExternalEmailDeliverySource {
        ExternalEmailDeliverySource {
            notification_delivery_id: NotificationDeliveryId::new(),
            external_email_message_id: ExternalEmailMessageId::new(),
            recipient: ExternalEmailRecipient::try_from("notifier@example.test")
                .unwrap_or_else(|error| panic!("invalid test recipient: {error}")),
            language,
            content: ExternalEmailContent::TestProbe(
                ExternalEmailTestProbe::new("probe-1")
                    .unwrap_or_else(|error| panic!("invalid test probe: {error}")),
            ),
        }
    }

    fn links(language: EmailLanguage) -> FrontendLinks<'static> {
        FrontendLinks::new("https://stage.aura-historia.com", language)
    }

    #[rstest]
    #[case(Language::De, "de")]
    #[case(Language::En, "en")]
    #[case(Language::Es, "es")]
    #[case(Language::Fr, "fr")]
    #[case(Language::It, "it")]
    #[case(Language::Ja, "en")]
    fn should_resolve_external_template_key_from_the_message_language(
        #[case] language: Language,
        #[case] expected_key_language: &str,
    ) {
        let source = external_source(language);

        assert_eq!(
            format!("mail/build-id/test/external-email/test-probe/{expected_key_language}.html"),
            s3_template_key(
                "mail/build-id/",
                external_template_type(&source.content),
                EmailLanguage::resolve(source.language),
            )
        );
    }

    #[test]
    fn should_give_external_templates_only_message_data_and_the_home_link() {
        let source = external_source(Language::Fr);

        assert_eq!(
            json!({
                "reference": "probe-1",
                "home_url": "https://stage.aura-historia.com/fr",
            }),
            external_template_data(&source, links(EmailLanguage::Fr))
        );
    }
}

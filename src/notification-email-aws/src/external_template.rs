//! Template selection and variables of external emails: transactional email to an address
//! that does not belong to a user account (`docs/events/flow.md`, External email delivery).

use crate::template_mapping::{EmailLanguage, EmailTemplateType, FrontendLinks};
use content_notice_core::reason_category::ReasonCategory;
use notification_core::external_email::{ContentNoticeReceipt, ExternalEmailContent};
use notification_service::ports::notification_delivery_repository::ExternalEmailDeliverySource;
use serde_json::{Value, json};
use time::{OffsetDateTime, UtcOffset};

pub(crate) const fn external_template_type(content: &ExternalEmailContent) -> EmailTemplateType {
    match content {
        ExternalEmailContent::TestProbe(_) => EmailTemplateType::ExternalEmailTestProbe,
        ExternalEmailContent::ContentNoticeReceipt(_) => EmailTemplateType::ContentNoticeReceipt,
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
        ExternalEmailContent::ContentNoticeReceipt(receipt) => {
            content_notice_receipt_data(receipt, links.language())
        }
    };
    data["home_url"] = json!(links.home());
    data
}

/// The receipt carries only data from Aura Historia's side (`docs/content-notices.md`,
/// Receipt email). `listing_title` is empty when there is no listing title to name.
fn content_notice_receipt_data(receipt: &ContentNoticeReceipt, language: EmailLanguage) -> Value {
    json!({
        "reference": receipt.content_notice_id.to_string(),
        "received_at": received_at_text(receipt.received_at, language),
        "provider_name": receipt.provider_name.as_ref(),
        "listing_title": receipt
            .listing_title
            .as_ref()
            .map_or("", |title| title.as_str()),
        "category_label": category_label(receipt.reason_category, language),
    })
}

/// The receipt time in UTC, written the way the email language writes dates and labelled
/// `UTC`, because the recipient's time zone is unknown.
fn received_at_text(received_at: OffsetDateTime, language: EmailLanguage) -> String {
    let utc = received_at.to_offset(UtcOffset::UTC);
    let day = utc.day();
    let month = month_name(utc.month(), language);
    let year = utc.year();
    let time = format!("{:02}:{:02} UTC", utc.hour(), utc.minute());
    match language {
        EmailLanguage::De => format!("{day}. {month} {year} um {time}"),
        EmailLanguage::En => format!("{day} {month} {year} at {time}"),
        EmailLanguage::Fr if day == 1 => format!("1er {month} {year} à {time}"),
        EmailLanguage::Fr => format!("{day} {month} {year} à {time}"),
        EmailLanguage::Es => format!("{day} de {month} de {year} a las {time}"),
        EmailLanguage::It if day == 1 => format!("1º {month} {year} alle {time}"),
        EmailLanguage::It => format!("{day} {month} {year} alle {time}"),
    }
}

fn month_name(month: time::Month, language: EmailLanguage) -> &'static str {
    const DE: [&str; 12] = [
        "Januar",
        "Februar",
        "März",
        "April",
        "Mai",
        "Juni",
        "Juli",
        "August",
        "September",
        "Oktober",
        "November",
        "Dezember",
    ];
    const EN: [&str; 12] = [
        "January",
        "February",
        "March",
        "April",
        "May",
        "June",
        "July",
        "August",
        "September",
        "October",
        "November",
        "December",
    ];
    const FR: [&str; 12] = [
        "janvier",
        "février",
        "mars",
        "avril",
        "mai",
        "juin",
        "juillet",
        "août",
        "septembre",
        "octobre",
        "novembre",
        "décembre",
    ];
    const ES: [&str; 12] = [
        "enero",
        "febrero",
        "marzo",
        "abril",
        "mayo",
        "junio",
        "julio",
        "agosto",
        "septiembre",
        "octubre",
        "noviembre",
        "diciembre",
    ];
    const IT: [&str; 12] = [
        "gennaio",
        "febbraio",
        "marzo",
        "aprile",
        "maggio",
        "giugno",
        "luglio",
        "agosto",
        "settembre",
        "ottobre",
        "novembre",
        "dicembre",
    ];
    let names = match language {
        EmailLanguage::De => &DE,
        EmailLanguage::En => &EN,
        EmailLanguage::Fr => &FR,
        EmailLanguage::Es => &ES,
        EmailLanguage::It => &IT,
    };
    names[usize::from(u8::from(month)) - 1]
}

fn category_label(category: ReasonCategory, language: EmailLanguage) -> &'static str {
    match language {
        EmailLanguage::De => match category {
            ReasonCategory::ProviderOptOut => "Der Anbieter möchte nicht gelistet werden",
            ReasonCategory::RightsInfringement => "Rechtsverletzung",
            ReasonCategory::IllegalContent => "Rechtswidrige Inhalte",
            ReasonCategory::IncorrectInformation => "Fehlerhafte Angaben",
            ReasonCategory::Other => "Sonstiges",
        },
        EmailLanguage::En => match category {
            ReasonCategory::ProviderOptOut => "The provider does not want to be listed",
            ReasonCategory::RightsInfringement => "Rights infringement",
            ReasonCategory::IllegalContent => "Illegal content",
            ReasonCategory::IncorrectInformation => "Incorrect information",
            ReasonCategory::Other => "Other",
        },
        EmailLanguage::Fr => match category {
            ReasonCategory::ProviderOptOut => "Le fournisseur ne souhaite pas être référencé",
            ReasonCategory::RightsInfringement => "Atteinte à des droits",
            ReasonCategory::IllegalContent => "Contenu illicite",
            ReasonCategory::IncorrectInformation => "Informations inexactes",
            ReasonCategory::Other => "Autre",
        },
        EmailLanguage::Es => match category {
            ReasonCategory::ProviderOptOut => "El proveedor no desea aparecer en el listado",
            ReasonCategory::RightsInfringement => "Infracción de derechos",
            ReasonCategory::IllegalContent => "Contenido ilícito",
            ReasonCategory::IncorrectInformation => "Información incorrecta",
            ReasonCategory::Other => "Otro",
        },
        EmailLanguage::It => match category {
            ReasonCategory::ProviderOptOut => "Il fornitore non desidera essere elencato",
            ReasonCategory::RightsInfringement => "Violazione di diritti",
            ReasonCategory::IllegalContent => "Contenuto illegale",
            ReasonCategory::IncorrectInformation => "Informazioni errate",
            ReasonCategory::Other => "Altro",
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::template_mapping::s3_template_key;
    use content_notice_core::{
        content_notice_id::ContentNoticeId, content_notice_target::ProductListingTitleSnapshot,
    };
    use listing_source_core::ListingSourceName;
    use localization::Language;
    use notification_core::{
        external_email::{ExternalEmailRecipient, ExternalEmailTestProbe},
        external_email_message_id::ExternalEmailMessageId,
        notification_delivery_id::NotificationDeliveryId,
    };
    use rstest::rstest;
    use strum::IntoEnumIterator;
    use time::macros::datetime;

    fn external_source(language: Language) -> ExternalEmailDeliverySource {
        source(
            language,
            ExternalEmailContent::TestProbe(
                ExternalEmailTestProbe::new("probe-1")
                    .unwrap_or_else(|error| panic!("invalid test probe: {error}")),
            ),
        )
    }

    fn source(language: Language, content: ExternalEmailContent) -> ExternalEmailDeliverySource {
        ExternalEmailDeliverySource {
            notification_delivery_id: NotificationDeliveryId::new(),
            external_email_message_id: ExternalEmailMessageId::new(),
            recipient: ExternalEmailRecipient::try_from("notifier@example.test")
                .unwrap_or_else(|error| panic!("invalid test recipient: {error}")),
            language,
            content,
        }
    }

    fn receipt(listing_title: Option<&str>) -> ContentNoticeReceipt {
        ContentNoticeReceipt {
            content_notice_id: ContentNoticeId::new(),
            received_at: datetime!(2026-11-14 13:05:59 +01:00),
            provider_name: ListingSourceName::try_from("Antik Shop")
                .unwrap_or_else(|error| panic!("invalid test name: {error}")),
            listing_title: listing_title.map(|title| {
                ProductListingTitleSnapshot::try_from(title)
                    .unwrap_or_else(|error| panic!("invalid test title: {error}"))
            }),
            reason_category: ReasonCategory::RightsInfringement,
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

    #[test]
    fn should_give_a_listing_receipt_only_its_reference_target_category_and_home_link() {
        let receipt = receipt(Some("Jugendstil-Vase"));
        let reference = receipt.content_notice_id.to_string();
        let source = source(
            Language::De,
            ExternalEmailContent::ContentNoticeReceipt(receipt),
        );

        assert_eq!(
            json!({
                "reference": reference,
                "received_at": "14. November 2026 um 12:05 UTC",
                "provider_name": "Antik Shop",
                "listing_title": "Jugendstil-Vase",
                "category_label": "Rechtsverletzung",
                "home_url": "https://stage.aura-historia.com/de",
            }),
            external_template_data(&source, links(EmailLanguage::De))
        );
        assert_eq!(
            "mail/build-id/mjml/content-notice/receipt/de.html",
            s3_template_key(
                "mail/build-id/",
                external_template_type(&source.content),
                EmailLanguage::De,
            )
        );
    }

    #[test]
    fn should_give_a_listing_source_receipt_an_empty_listing_title() {
        let source = source(
            Language::En,
            ExternalEmailContent::ContentNoticeReceipt(receipt(None)),
        );

        assert_eq!(
            json!(""),
            external_template_data(&source, links(EmailLanguage::En))["listing_title"]
        );
    }

    #[rstest]
    #[case(EmailLanguage::De, "1. Januar 2027 um 00:00 UTC")]
    #[case(EmailLanguage::En, "1 January 2027 at 00:00 UTC")]
    #[case(EmailLanguage::Fr, "1er janvier 2027 à 00:00 UTC")]
    #[case(EmailLanguage::Es, "1 de enero de 2027 a las 00:00 UTC")]
    #[case(EmailLanguage::It, "1º gennaio 2027 alle 00:00 UTC")]
    fn should_write_the_receipt_time_in_utc_for_the_email_language(
        #[case] language: EmailLanguage,
        #[case] expected: &str,
    ) {
        assert_eq!(
            expected,
            received_at_text(datetime!(2026-12-31 19:00:30 -05:00), language)
        );
    }

    #[test]
    fn should_name_every_month_distinctly_in_every_email_language() {
        for language in [
            EmailLanguage::De,
            EmailLanguage::En,
            EmailLanguage::Fr,
            EmailLanguage::Es,
            EmailLanguage::It,
        ] {
            let names =
                std::iter::successors(Some(time::Month::January), |month| Some(month.next()))
                    .take(12)
                    .map(|month| month_name(month, language))
                    .collect::<std::collections::HashSet<_>>();
            assert_eq!(12, names.len(), "{language:?}");
        }
        assert_eq!(
            "Dezember",
            month_name(time::Month::December, EmailLanguage::De)
        );
    }

    #[test]
    fn should_label_every_reason_category_distinctly_in_every_email_language() {
        for language in [
            EmailLanguage::De,
            EmailLanguage::En,
            EmailLanguage::Fr,
            EmailLanguage::Es,
            EmailLanguage::It,
        ] {
            let labels = ReasonCategory::iter()
                .map(|category| category_label(category, language))
                .collect::<std::collections::HashSet<_>>();
            assert_eq!(ReasonCategory::iter().count(), labels.len(), "{language:?}");
        }
    }
}

use crate::{
    provider_failure::{classify_s3_template_fetch, provider_error},
    template_mapping::{EmailLanguage, EmailTemplateType, s3_template_key},
};
use application::error::box_error;
use aws_sdk_s3::Client as S3Client;
use handlebars::Handlebars;
use notification_service::ports::notification_channel_sender::NotificationChannelSendError;
use serde_json::Value;

pub(crate) struct TemplateReader {
    s3: S3Client,
    template_bucket: String,
    template_prefix: String,
    handlebars: Handlebars<'static>,
}

impl TemplateReader {
    pub(crate) fn new(
        s3: S3Client,
        template_bucket: impl Into<String>,
        template_prefix: impl Into<String>,
    ) -> Self {
        Self {
            s3,
            template_bucket: template_bucket.into(),
            template_prefix: template_prefix.into(),
            handlebars: Handlebars::new(),
        }
    }

    pub(crate) async fn render(
        &self,
        template_type: EmailTemplateType,
        language: EmailLanguage,
        data: &Value,
    ) -> Result<String, NotificationChannelSendError> {
        let key = s3_template_key(&self.template_prefix, template_type, language);
        let response = self
            .s3
            .get_object()
            .bucket(&self.template_bucket)
            .key(key)
            .send()
            .await
            .map_err(|source| {
                let template_missing = source
                    .as_service_error()
                    .is_some_and(|error| error.is_no_such_key());
                let status_code = source
                    .raw_response()
                    .map(|response| response.status().as_u16());
                provider_error(
                    classify_s3_template_fetch(template_missing, status_code),
                    box_error(source),
                )
            })?;
        let bytes = response
            .body
            .collect()
            .await
            .map_err(|source| NotificationChannelSendError::Retryable {
                code: "S3_TEMPLATE_READ_FAILED",
                source: box_error(source),
            })?
            .into_bytes();
        let template = String::from_utf8(bytes.to_vec()).map_err(|source| {
            NotificationChannelSendError::Permanent {
                code: "S3_TEMPLATE_INVALID_UTF8",
                source: box_error(source),
            }
        })?;
        render_template(&self.handlebars, &template, data)
    }
}

fn render_template(
    handlebars: &Handlebars<'static>,
    template: &str,
    data: &Value,
) -> Result<String, NotificationChannelSendError> {
    handlebars
        .render_template(template, data)
        .map_err(|source| NotificationChannelSendError::Permanent {
            code: "S3_TEMPLATE_RENDER_FAILED",
            source: box_error(source),
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn should_return_safe_code_when_template_cannot_render() {
        assert!(matches!(
            render_template(&Handlebars::new(), "{{#if", &json!({})),
            Err(NotificationChannelSendError::Permanent {
                code: "S3_TEMPLATE_RENDER_FAILED",
                ..
            })
        ));
    }

    #[test]
    fn should_render_every_localized_product_listing_template_with_delivery_contract_data()
    -> Result<(), NotificationChannelSendError> {
        let mut handlebars = Handlebars::new();
        handlebars.set_strict_mode(true);
        let data = json!({
            "first_name": "Ada",
            "listing_source_name": "Aster Antiques",
            "listing_source_partner": true,
            "title": "Bronze Vase",
            "image_url": "https://images.example.test/bronze-vase.jpg",
            "product_listing_url": "https://aura-historia.com/en/products/bronze-vase-a1b2c3",
            "home_url": "https://aura-historia.com/en",
            "watchlist_url": "https://aura-historia.com/en/me/watchlist",
            "search_filter_url": "https://aura-historia.com/en/me/search-filter/a1b2c3d4",
            "search_filter_id": "a1b2c3d4",
            "search_filter_name": "Art Nouveau",
            "old_availability": "Available",
            "new_availability": "In stock",
            "old_price": "100,00 €",
            "new_price": "90,00 €",
        });

        for contract in product_listing_template_contracts() {
            let rendered = render_template(&handlebars, contract.template, &data)?;

            assert!(
                !rendered.contains("{{"),
                "{} left an unresolved Handlebars expression",
                contract.path
            );
            for marker in contract.required_markers {
                assert!(
                    rendered.contains(marker),
                    "{} did not render required delivery value {marker:?}",
                    contract.path
                );
            }
            for link in rendered.split("<a href=\"").skip(1) {
                assert!(
                    link.starts_with("https://aura-historia.com/") || link.starts_with("mailto:"),
                    "{} links outside Aura Historia: {}",
                    contract.path,
                    link.split('"').next().unwrap_or_default()
                );
            }
            let partner_tag = partner_tag(contract.path);
            assert!(
                rendered.contains(partner_tag),
                "{} did not tag a partner with {partner_tag:?}",
                contract.path
            );
            let mut non_partner = data.clone();
            non_partner["listing_source_partner"] = json!(false);
            assert!(
                !render_template(&handlebars, contract.template, &non_partner)?.contains('✓'),
                "{} tagged a non-partner",
                contract.path
            );
        }

        Ok(())
    }

    /// The webapp badge's label in each e-mail language; never "verified".
    fn partner_tag(path: &str) -> &'static str {
        match path.rsplit('/').next() {
            Some("fr.mjml") => "✓ PARTENAIRE",
            Some("es.mjml") => "✓ SOCIO",
            Some("de.mjml" | "en.mjml" | "it.mjml") => "✓ PARTNER",
            _ => panic!("{path} is not a localized e-mail template"),
        }
    }

    #[rstest::rstest]
    #[case("de")]
    #[case("en")]
    #[case("es")]
    #[case("fr")]
    #[case("it")]
    fn should_tag_only_partner_cards_in_the_digest(
        #[case] language: &str,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let path = format!("mjml/search-filter/digest/{language}.mjml");
        let template = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../..")
                .join(&path),
        )?;
        let mut handlebars = Handlebars::new();
        handlebars.set_strict_mode(true);
        let card = |name: &str, partner: bool| {
            json!({
                "listing_source_name": name,
                "listing_source_partner": partner,
                "product_listing_url": "https://aura-historia.com/en/products/bronze-vase-a1b2c3",
                "title": "Bronze Vase",
                "image_url": null,
                "searches": [{ "id": "f1", "name": "Art Nouveau", "url": "https://aura-historia.com/en/me/search-filter/f1" }],
            })
        };
        let rendered = render_template(
            &handlebars,
            &template,
            &json!({
                "first_name": null,
                "home_url": "https://aura-historia.com/en",
                "match_count": 2,
                "listing_count": 2,
                "preview_count": 2,
                "preview_limit": 20,
                "has_more": false,
                "groups": [{ "id": "f1", "name": "Art Nouveau", "url": "https://aura-historia.com/en/me/search-filter/f1", "match_count": 2 }],
                "listings": [card("Aster Antiques", true), card("Brocante du Midi", false)],
                "results_url": "https://aura-historia.com/en/me/search-filters",
                "settings_url": "https://aura-historia.com/en/me/search-filters",
            }),
        )?;

        assert_eq!(1, rendered.matches(partner_tag(&path)).count());
        assert_eq!(1, rendered.matches('✓').count());
        let tagged = rendered
            .split("Aster Antiques")
            .nth(1)
            .ok_or("partner card rendered")?;
        assert!(
            tagged
                .split("Brocante du Midi")
                .next()
                .is_some_and(|card| card.contains('✓'))
        );
        Ok(())
    }

    struct ProductListingTemplateContract {
        path: &'static str,
        template: &'static str,
        required_markers: &'static [&'static str],
    }

    fn product_listing_template_contracts() -> [ProductListingTemplateContract; 15] {
        const SEARCH_FILTER_MARKERS: &[&str] = &[
            "Ada",
            "Aster Antiques",
            "Bronze Vase",
            "https://images.example.test/bronze-vase.jpg",
            "https://aura-historia.com/en/products/bronze-vase-a1b2c3",
            "https://aura-historia.com/en\"",
            "https://aura-historia.com/en/me/search-filter/a1b2c3d4",
            "Art Nouveau",
        ];
        const AVAILABILITY_MARKERS: &[&str] = &[
            "Ada",
            "Aster Antiques",
            "Bronze Vase",
            "https://images.example.test/bronze-vase.jpg",
            "https://aura-historia.com/en/products/bronze-vase-a1b2c3",
            "https://aura-historia.com/en\"",
            "https://aura-historia.com/en/me/watchlist",
            "Available",
            "In stock",
        ];
        const PRICE_MARKERS: &[&str] = &[
            "Ada",
            "Aster Antiques",
            "Bronze Vase",
            "https://images.example.test/bronze-vase.jpg",
            "https://aura-historia.com/en/products/bronze-vase-a1b2c3",
            "https://aura-historia.com/en\"",
            "https://aura-historia.com/en/me/watchlist",
            "100,00 €",
            "90,00 €",
        ];

        [
            ProductListingTemplateContract {
                path: "mjml/search-filter/match/de.mjml",
                template: include_str!("../../../mjml/search-filter/match/de.mjml"),
                required_markers: SEARCH_FILTER_MARKERS,
            },
            ProductListingTemplateContract {
                path: "mjml/search-filter/match/en.mjml",
                template: include_str!("../../../mjml/search-filter/match/en.mjml"),
                required_markers: SEARCH_FILTER_MARKERS,
            },
            ProductListingTemplateContract {
                path: "mjml/search-filter/match/es.mjml",
                template: include_str!("../../../mjml/search-filter/match/es.mjml"),
                required_markers: SEARCH_FILTER_MARKERS,
            },
            ProductListingTemplateContract {
                path: "mjml/search-filter/match/fr.mjml",
                template: include_str!("../../../mjml/search-filter/match/fr.mjml"),
                required_markers: SEARCH_FILTER_MARKERS,
            },
            ProductListingTemplateContract {
                path: "mjml/search-filter/match/it.mjml",
                template: include_str!("../../../mjml/search-filter/match/it.mjml"),
                required_markers: SEARCH_FILTER_MARKERS,
            },
            ProductListingTemplateContract {
                path: "mjml/watchlist/product-update/availability/de.mjml",
                template: include_str!(
                    "../../../mjml/watchlist/product-update/availability/de.mjml"
                ),
                required_markers: AVAILABILITY_MARKERS,
            },
            ProductListingTemplateContract {
                path: "mjml/watchlist/product-update/availability/en.mjml",
                template: include_str!(
                    "../../../mjml/watchlist/product-update/availability/en.mjml"
                ),
                required_markers: AVAILABILITY_MARKERS,
            },
            ProductListingTemplateContract {
                path: "mjml/watchlist/product-update/availability/es.mjml",
                template: include_str!(
                    "../../../mjml/watchlist/product-update/availability/es.mjml"
                ),
                required_markers: AVAILABILITY_MARKERS,
            },
            ProductListingTemplateContract {
                path: "mjml/watchlist/product-update/availability/fr.mjml",
                template: include_str!(
                    "../../../mjml/watchlist/product-update/availability/fr.mjml"
                ),
                required_markers: AVAILABILITY_MARKERS,
            },
            ProductListingTemplateContract {
                path: "mjml/watchlist/product-update/availability/it.mjml",
                template: include_str!(
                    "../../../mjml/watchlist/product-update/availability/it.mjml"
                ),
                required_markers: AVAILABILITY_MARKERS,
            },
            ProductListingTemplateContract {
                path: "mjml/watchlist/product-update/price/de.mjml",
                template: include_str!("../../../mjml/watchlist/product-update/price/de.mjml"),
                required_markers: PRICE_MARKERS,
            },
            ProductListingTemplateContract {
                path: "mjml/watchlist/product-update/price/en.mjml",
                template: include_str!("../../../mjml/watchlist/product-update/price/en.mjml"),
                required_markers: PRICE_MARKERS,
            },
            ProductListingTemplateContract {
                path: "mjml/watchlist/product-update/price/es.mjml",
                template: include_str!("../../../mjml/watchlist/product-update/price/es.mjml"),
                required_markers: PRICE_MARKERS,
            },
            ProductListingTemplateContract {
                path: "mjml/watchlist/product-update/price/fr.mjml",
                template: include_str!("../../../mjml/watchlist/product-update/price/fr.mjml"),
                required_markers: PRICE_MARKERS,
            },
            ProductListingTemplateContract {
                path: "mjml/watchlist/product-update/price/it.mjml",
                template: include_str!("../../../mjml/watchlist/product-update/price/it.mjml"),
                required_markers: PRICE_MARKERS,
            },
        ]
    }

    const PARTNERSHIP_TEMPLATES: [(&str, &str); 10] = [
        (
            "approval/de",
            include_str!("../../../mjml/partnership-application/approval/de.mjml"),
        ),
        (
            "approval/en",
            include_str!("../../../mjml/partnership-application/approval/en.mjml"),
        ),
        (
            "approval/es",
            include_str!("../../../mjml/partnership-application/approval/es.mjml"),
        ),
        (
            "approval/fr",
            include_str!("../../../mjml/partnership-application/approval/fr.mjml"),
        ),
        (
            "approval/it",
            include_str!("../../../mjml/partnership-application/approval/it.mjml"),
        ),
        (
            "rejection/de",
            include_str!("../../../mjml/partnership-application/rejection/de.mjml"),
        ),
        (
            "rejection/en",
            include_str!("../../../mjml/partnership-application/rejection/en.mjml"),
        ),
        (
            "rejection/es",
            include_str!("../../../mjml/partnership-application/rejection/es.mjml"),
        ),
        (
            "rejection/fr",
            include_str!("../../../mjml/partnership-application/rejection/fr.mjml"),
        ),
        (
            "rejection/it",
            include_str!("../../../mjml/partnership-application/rejection/it.mjml"),
        ),
    ];

    #[test]
    fn should_render_every_partnership_template_with_delivery_contract_data()
    -> Result<(), NotificationChannelSendError> {
        let mut handlebars = Handlebars::new();
        handlebars.set_strict_mode(true);
        let data = json!({
            "first_name": "Ada",
            "party_name": "Aster Antiques GmbH",
            "listing_source_name": "aster-antiques.example",
            "image_url": null,
            "home_url": "https://aura-historia.com/en",
            "partner_dashboard_url": "https://aura-historia.com/en/partners/applications",
            "notification_type": "partnership_application_approval",
        });

        for (path, template) in PARTNERSHIP_TEMPLATES {
            let rendered = render_template(&handlebars, template, &data)?;
            assert!(
                !rendered.contains("{{"),
                "{path} left an unresolved expression"
            );
            for marker in ["Ada", "Aster Antiques GmbH", "aster-antiques.example"] {
                assert!(
                    rendered.contains(marker),
                    "{path} did not render {marker:?}"
                );
            }
            assert_eq!(
                path.starts_with("approval"),
                rendered.contains("https://aura-historia.com/en/partners/applications"),
                "{path} links the partner dashboard only on approval"
            );
        }
        Ok(())
    }

    /// Stands in for the test-only external kind, which has no deployed template, so the
    /// checks below run for the mechanism. Real external kinds list their MJML files.
    const EXTERNAL_TEST_PROBE_TEMPLATE: &str = "<html><body><a href=\"{{home_url}}\">Aura Historia</a><p>Reference: {{reference}}</p><a href=\"mailto:contact@aura-historia.com\">contact@aura-historia.com</a></body></html>";

    fn external_templates() -> [(EmailTemplateType, [&'static str; 5]); 1] {
        [(
            EmailTemplateType::ExternalEmailTestProbe,
            [EXTERNAL_TEST_PROBE_TEMPLATE; 5],
        )]
    }

    #[test]
    fn should_render_every_external_template_in_strict_mode_without_account_links()
    -> Result<(), Box<dyn std::error::Error>> {
        use crate::template_mapping::{FrontendLinks, external_template_data};
        use localization::Language;
        use notification_core::{
            external_email::{ExternalEmailContent, ExternalEmailTestProbe},
            external_email_message_id::ExternalEmailMessageId,
            notification_delivery_id::NotificationDeliveryId,
        };
        use notification_service::ports::notification_delivery_repository::ExternalEmailDeliverySource;

        let mut handlebars = Handlebars::new();
        handlebars.set_strict_mode(true);
        let languages = [
            EmailLanguage::De,
            EmailLanguage::En,
            EmailLanguage::Es,
            EmailLanguage::Fr,
            EmailLanguage::It,
        ];
        for (template_type, templates) in external_templates() {
            // Ja has no email language and falls back to the English template.
            for language in [
                Language::De,
                Language::En,
                Language::Es,
                Language::Fr,
                Language::It,
                Language::Ja,
            ] {
                let email_language = EmailLanguage::resolve(language);
                let template = languages
                    .iter()
                    .position(|candidate| *candidate == email_language)
                    .map(|index| templates[index])
                    .ok_or("every email language has a template")?;
                let source = ExternalEmailDeliverySource {
                    notification_delivery_id: NotificationDeliveryId::new(),
                    external_email_message_id: ExternalEmailMessageId::new(),
                    recipient: serde_email::Email::new("notifier@example.test")?,
                    language,
                    content: ExternalEmailContent::TestProbe(ExternalEmailTestProbe::new(
                        "probe-1",
                    )?),
                };
                let data = external_template_data(
                    &source,
                    FrontendLinks::new("https://aura-historia.com", email_language),
                );

                let rendered = render_template(&handlebars, template, &data)?;

                assert!(
                    !rendered.contains("{{"),
                    "{template_type:?} left an expression"
                );
                assert!(
                    rendered.contains("probe-1"),
                    "{template_type:?} lost its data"
                );
                for link in rendered.split("<a href=\"").skip(1) {
                    let link = link.split('"').next().unwrap_or_default();
                    assert!(
                        link.starts_with("https://aura-historia.com/")
                            || link.starts_with("mailto:"),
                        "{template_type:?} links outside Aura Historia: {link}"
                    );
                    assert!(
                        !link.contains("/me/") && !link.ends_with("/me"),
                        "{template_type:?} links to an account page: {link}"
                    );
                }
            }
        }
        Ok(())
    }

    #[rstest::rstest]
    #[case("en", 1, 1, "1 new match", "in 1 listing")]
    #[case("en", 3, 1, "3 new matches", "in 1 listing")]
    #[case("en", 3, 2, "3 new matches", "across 2 listings")]
    #[case("de", 1, 1, "1 neuer Treffer", "in 1 Angebot")]
    #[case("de", 3, 2, "3 neue Treffer", "in 2 Angeboten")]
    #[case("fr", 1, 1, "1 nouveau résultat", "dans 1 annonce")]
    #[case("fr", 3, 2, "3 nouveaux résultats", "parmi 2 annonces")]
    #[case("es", 1, 1, "1 nuevo resultado", "en 1 anuncio")]
    #[case("es", 3, 2, "3 nuevos resultados", "en 2 anuncios")]
    #[case("it", 1, 1, "1 nuovo risultato", "in 1 annuncio")]
    #[case("it", 3, 2, "3 nuovi risultati", "in 2 annunci")]
    fn should_pluralize_digest_counts(
        #[case] language: &str,
        #[case] match_count: u64,
        #[case] listing_count: u64,
        #[case] matches: &str,
        #[case] listings: &str,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let template = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join(format!("../../mjml/search-filter/digest/{language}.mjml")),
        )?;
        let mut handlebars = Handlebars::new();
        handlebars.set_strict_mode(true);
        let rendered = render_template(
            &handlebars,
            &template,
            &json!({
                "first_name": null,
                "home_url": "https://aura-historia.com/en",
                "match_count": match_count,
                "listing_count": listing_count,
                "preview_count": listing_count,
                "preview_limit": 20,
                "has_more": false,
                "groups": [{ "id": "f1", "name": "Art Nouveau", "url": "https://aura-historia.com/en/me/search-filter/f1", "match_count": match_count }],
                "listings": [],
                "results_url": "https://aura-historia.com/en/me/search-filters",
                "settings_url": "https://aura-historia.com/en/me/search-filters",
            }),
        )?;
        assert!(rendered.contains(&format!("{matches} <em>{listings}.</em>")));
        assert!(!rendered.contains("{{"));
        Ok(())
    }
}

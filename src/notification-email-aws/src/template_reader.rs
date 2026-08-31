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
    stage: String,
    commit_sha: String,
    handlebars: Handlebars<'static>,
}

impl TemplateReader {
    pub(crate) fn new(
        s3: S3Client,
        template_bucket: impl Into<String>,
        stage: impl Into<String>,
        commit_sha: impl Into<String>,
    ) -> Self {
        Self {
            s3,
            template_bucket: template_bucket.into(),
            stage: stage.into(),
            commit_sha: commit_sha.into(),
            handlebars: Handlebars::new(),
        }
    }

    pub(crate) async fn render(
        &self,
        template_type: EmailTemplateType,
        language: EmailLanguage,
        data: &Value,
    ) -> Result<String, NotificationChannelSendError> {
        let key = s3_template_key(&self.stage, &self.commit_sha, template_type, language);
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
    fn should_render_template_with_notification_data() -> Result<(), NotificationChannelSendError> {
        let rendered = render_template(
            &Handlebars::new(),
            "Hello {{listing_source_name}}. New price: {{new_price}}.",
            &json!({ "listing_source_name": "Aster Antiques", "new_price": "12,00 €" }),
        )?;
        assert_eq!(rendered, "Hello Aster Antiques. New price: 12,00 €.");
        Ok(())
    }

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
            "title": "Bronze Vase",
            "image_url": "https://images.example.test/bronze-vase.jpg",
            "product_listing_url": "https://aura-historia.com/product-listings/bronze-vase-a1b2c3",
            "view_url": "https://merchant.example.test/products/bronze-vase",
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
        }

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
            "https://aura-historia.com/product-listings/bronze-vase-a1b2c3",
            "https://merchant.example.test/products/bronze-vase",
            "a1b2c3d4",
            "Art Nouveau",
        ];
        const AVAILABILITY_MARKERS: &[&str] = &[
            "Ada",
            "Aster Antiques",
            "Bronze Vase",
            "https://images.example.test/bronze-vase.jpg",
            "https://aura-historia.com/product-listings/bronze-vase-a1b2c3",
            "https://merchant.example.test/products/bronze-vase",
            "Available",
            "In stock",
        ];
        const PRICE_MARKERS: &[&str] = &[
            "Ada",
            "Aster Antiques",
            "Bronze Vase",
            "https://images.example.test/bronze-vase.jpg",
            "https://aura-historia.com/product-listings/bronze-vase-a1b2c3",
            "https://merchant.example.test/products/bronze-vase",
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
}

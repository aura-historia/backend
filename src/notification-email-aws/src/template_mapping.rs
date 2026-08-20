use common::{language::domain::Language, price::domain::Price};
use notification_core::{
    notification::{
        LocalizedNotificationContent, LocalizedNotificationWatchlistChange, NotificationContent,
        NotificationWatchlistChange, PartnerApplicationDecision,
    },
    presentation::present_image,
};
use notification_service::ports::notification_delivery_repository::NotificationDeliverySource;
use serde_json::{Value, json};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EmailTemplateType {
    WatchlistUpdatePrice,
    WatchlistUpdateState,
    SearchFilterMatch,
    PartnerApplicationApproval,
    PartnerApplicationRejection,
}

pub(crate) const fn template_type(content: &NotificationContent) -> EmailTemplateType {
    match content {
        NotificationContent::Watchlist {
            change: NotificationWatchlistChange::PriceChange { .. },
            ..
        } => EmailTemplateType::WatchlistUpdatePrice,
        NotificationContent::Watchlist { .. } => EmailTemplateType::WatchlistUpdateState,
        NotificationContent::SearchFilter { .. } => EmailTemplateType::SearchFilterMatch,
        NotificationContent::PartnerApplication {
            decision: PartnerApplicationDecision::Approved,
            ..
        } => EmailTemplateType::PartnerApplicationApproval,
        NotificationContent::PartnerApplication { .. } => {
            EmailTemplateType::PartnerApplicationRejection
        }
    }
}

pub(crate) const fn template_directory(template_type: EmailTemplateType) -> &'static str {
    match template_type {
        EmailTemplateType::WatchlistUpdatePrice => "mjml/watchlist/product-update/price",
        EmailTemplateType::WatchlistUpdateState => "mjml/watchlist/product-update/state",
        EmailTemplateType::SearchFilterMatch => "mjml/search-filter/match",
        EmailTemplateType::PartnerApplicationApproval => "mjml/partner-application/approval",
        EmailTemplateType::PartnerApplicationRejection => "mjml/partner-application/rejection",
    }
}

pub(crate) fn s3_template_key(
    stage: &str,
    commit_sha: &str,
    template_type: EmailTemplateType,
    language: Language,
) -> String {
    format!(
        "{stage}/{commit_sha}/{}/{}.html",
        template_directory(template_type),
        language.as_str()
    )
}

pub(crate) const fn ses_template_tag_value(template_type: EmailTemplateType) -> &'static str {
    match template_type {
        EmailTemplateType::WatchlistUpdatePrice => "WATCHLIST_UPDATE_PRICE",
        EmailTemplateType::WatchlistUpdateState => "WATCHLIST_UPDATE_STATE",
        EmailTemplateType::SearchFilterMatch => "SEARCH_FILTER_MATCH",
        EmailTemplateType::PartnerApplicationApproval => "PARTNER_APPLICATION_APPROVAL",
        EmailTemplateType::PartnerApplicationRejection => "PARTNER_APPLICATION_REJECTION",
    }
}

pub(crate) const fn subject(template_type: EmailTemplateType, language: Language) -> &'static str {
    match language {
        Language::De => match template_type {
            EmailTemplateType::WatchlistUpdatePrice => {
                "Der Preis auf deiner Merkliste hat sich geändert"
            }
            EmailTemplateType::WatchlistUpdateState => {
                "Ein Artikel auf deiner Merkliste hat sich geändert"
            }
            EmailTemplateType::SearchFilterMatch => "Neuer Treffer für deinen Suchfilter",
            EmailTemplateType::PartnerApplicationApproval => "Partnerantrag genehmigt",
            EmailTemplateType::PartnerApplicationRejection => "Update zu deinem Partnerantrag",
        },
        _ => match template_type {
            EmailTemplateType::WatchlistUpdatePrice => "Your watchlist price changed",
            EmailTemplateType::WatchlistUpdateState => "Your watchlist item changed",
            EmailTemplateType::SearchFilterMatch => "New search filter match",
            EmailTemplateType::PartnerApplicationApproval => "Partner application approved",
            EmailTemplateType::PartnerApplicationRejection => "Partner application update",
        },
    }
}

pub(crate) fn template_data(
    source: &NotificationDeliverySource,
    first_name: Option<&str>,
) -> Value {
    let localized = source.content.clone().localized(
        &source.presentation_preferences.currency,
        &[source.presentation_preferences.language],
    );
    let present_image_url = |image| {
        present_image(
            image,
            source.presentation_preferences.prohibited_content_consent,
        )
        .and_then(|image| image.url)
        .map(|url| url.to_string())
    };
    match localized {
        LocalizedNotificationContent::Watchlist {
            snapshot, change, ..
        } => {
            let mut data = product_template_data(
                snapshot.shop_name.to_string(),
                snapshot.shop_slug_id.to_string(),
                snapshot.product_slug_id.to_string(),
                snapshot.title.map(|title| title.payload.to_string()),
                present_image_url(snapshot.image),
                snapshot.view_url.to_string(),
            );
            match change {
                LocalizedNotificationWatchlistChange::PriceChange {
                    old_price,
                    new_price,
                } => {
                    data["old_price"] = json!(price_text(old_price));
                    data["new_price"] = json!(price_text(new_price));
                    data["notification_type"] = json!("price_change");
                }
                LocalizedNotificationWatchlistChange::StateChange {
                    old_state,
                    new_state,
                } => {
                    data["old_state"] = json!(state_text(
                        old_state,
                        source.presentation_preferences.language
                    ));
                    data["new_state"] = json!(state_text(
                        new_state,
                        source.presentation_preferences.language
                    ));
                    data["notification_type"] = json!("state_change");
                }
            }
            add_recipient_data(data, first_name)
        }
        LocalizedNotificationContent::SearchFilter {
            snapshot,
            user_search_filter_id,
            user_search_filter_name,
            ..
        } => {
            let mut data = product_template_data(
                snapshot.shop_name.to_string(),
                snapshot.shop_slug_id.to_string(),
                snapshot.product_slug_id.to_string(),
                snapshot.title.map(|title| title.payload.to_string()),
                present_image_url(snapshot.image),
                snapshot.view_url.to_string(),
            );
            data["search_filter_id"] = json!(user_search_filter_id.to_string());
            data["search_filter_name"] = json!(user_search_filter_name.to_string());
            data["notification_type"] = json!("search_filter_match");
            add_recipient_data(data, first_name)
        }
        LocalizedNotificationContent::PartnerApplication {
            snapshot, decision, ..
        } => add_recipient_data(
            json!({
                "shop_name": snapshot.shop_name.to_string(),
                "image_url": snapshot.image.map(|image| image.to_string()),
                "notification_type": match decision {
                    PartnerApplicationDecision::Approved => "partner_application_approval",
                    PartnerApplicationDecision::Rejected => "partner_application_rejection",
                },
            }),
            first_name,
        ),
    }
}

fn add_recipient_data(mut data: Value, first_name: Option<&str>) -> Value {
    data["first_name"] = json!(first_name);
    data
}
fn product_template_data(
    shop_name: String,
    shop_slug_id: String,
    product_slug_id: String,
    title: Option<String>,
    image_url: Option<String>,
    view_url: String,
) -> Value {
    json!({ "shop_name": shop_name, "shop_slug_id": shop_slug_id, "product_slug_id": product_slug_id, "title": title, "image_url": image_url, "view_url": view_url })
}
fn price_text(price: Option<Price>) -> Option<String> {
    price.map(|price| price.format_human_readable())
}

fn state_text(
    state: common::product_state::domain::ProductState,
    language: Language,
) -> &'static str {
    state.format_human_readable(&language)
}

#[cfg(test)]
mod tests {
    use super::*;
    use common::product_state::domain::ProductState;
    use common::{
        currency::domain::Currency, event_id::EventId, language::domain::Language,
        notification_id::NotificationId, product_id::ProductId, product_slug_id::ProductSlugId,
        shop_id::ShopId, shop_name::ShopName, shop_slug_id::ShopSlugId,
        shops_product_id::ShopsProductId, user_id::UserId,
    };
    use notification_core::{
        notification::{
            NotificationContent, NotificationWatchlistChange, ProductNotificationSnapshot,
        },
        notification_delivery::{NotificationDeliveryChannel, NotificationDeliveryTargetKey},
        notification_delivery_id::NotificationDeliveryId,
    };
    use notification_service::{
        ports::notification_delivery_repository::NotificationDeliverySource,
        presentation::NotificationPresentationPreferences,
    };
    use rstest::rstest;
    use url::Url;

    #[rstest]
    #[case(
        EmailTemplateType::WatchlistUpdatePrice,
        "Your watchlist price changed",
        "Der Preis auf deiner Merkliste hat sich geändert"
    )]
    #[case(
        EmailTemplateType::WatchlistUpdateState,
        "Your watchlist item changed",
        "Ein Artikel auf deiner Merkliste hat sich geändert"
    )]
    #[case(
        EmailTemplateType::SearchFilterMatch,
        "New search filter match",
        "Neuer Treffer für deinen Suchfilter"
    )]
    #[case(
        EmailTemplateType::PartnerApplicationApproval,
        "Partner application approved",
        "Partnerantrag genehmigt"
    )]
    #[case(
        EmailTemplateType::PartnerApplicationRejection,
        "Partner application update",
        "Update zu deinem Partnerantrag"
    )]
    fn should_localize_subject_for_every_email_template(
        #[case] template: EmailTemplateType,
        #[case] expected_en: &str,
        #[case] expected_de: &str,
    ) {
        assert_eq!(expected_en, subject(template, Language::En));
        assert_eq!(expected_de, subject(template, Language::De));
    }

    #[rstest]
    #[case(Language::En, "Listed", "Available")]
    #[case(Language::De, "Gelistet", "Verfügbar")]
    fn should_localize_watchlist_state_change_for_recipient_language(
        #[case] language: Language,
        #[case] expected_old_state: &str,
        #[case] expected_new_state: &str,
    ) {
        assert_eq!(
            expected_old_state,
            state_text(ProductState::Listed, language)
        );
        assert_eq!(
            expected_new_state,
            state_text(ProductState::Available, language)
        );
    }

    #[rstest]
    #[case(Some("None"), false, Some("https://shop.example/image.jpg"))]
    #[case(Some("NaziGermany"), false, None)]
    #[case(Some("NaziGermany"), true, Some("https://shop.example/image.jpg"))]
    #[case(None, false, None)]
    fn should_filter_image_url_in_email_template_data(
        #[case] prohibited_content: Option<&str>,
        #[case] consent: bool,
        #[case] expected_image_url: Option<&str>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let source = source(consent, prohibited_content)?;
        let data = template_data(&source, None);

        assert_eq!(expected_image_url, data["image_url"].as_str());
        Ok(())
    }

    fn source(
        prohibited_content_consent: bool,
        prohibited_content: Option<&str>,
    ) -> Result<NotificationDeliverySource, Box<dyn std::error::Error>> {
        Ok(NotificationDeliverySource {
            notification_delivery_id: NotificationDeliveryId::new(),
            notification_id: NotificationId::new(),
            user_id: UserId::new(),
            channel: NotificationDeliveryChannel::Email,
            target_key: NotificationDeliveryTargetKey::primary(),
            content: NotificationContent::Watchlist {
                origin_event_id: EventId::new(),
                product_id: ProductId::new(),
                snapshot: ProductNotificationSnapshot {
                    shop_id: ShopId::new(),
                    shops_product_id: ShopsProductId::from("shop-product"),
                    shop_slug_id: ShopSlugId::from("shop"),
                    product_slug_id: ProductSlugId::from("product"),
                    shop_name: ShopName::from("Test Shop"),
                    title: None,
                    image: prohibited_content
                        .map(|prohibited_content| {
                            serde_json::from_value(serde_json::json!({
                                "url": "https://shop.example/image.jpg",
                                "prohibited_content": prohibited_content,
                            }))
                        })
                        .transpose()?,
                    url: Url::parse("https://shop.example/product")?,
                    view_url: Url::parse("https://aura-historia.example/product")?,
                },
                change: NotificationWatchlistChange::StateChange {
                    old_state: ProductState::Listed,
                    new_state: ProductState::Available,
                },
            },
            presentation_preferences: NotificationPresentationPreferences {
                language: Language::En,
                currency: Currency::Eur,
                prohibited_content_consent,
            },
        })
    }
}

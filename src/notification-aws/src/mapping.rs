use common::{language::domain::Language, price::domain::Price};
use notification_core::{
    mail_template::MailTemplateType,
    notification::{
        LocalizedNotificationContent, LocalizedNotificationWatchlistChange, NotificationContent,
        NotificationWatchlistChange, PartnerApplicationDecision,
    },
};
use notification_service::ports::notification_delivery_repository::NotificationDeliverySource;
use serde_json::{Value, json};

pub(crate) const fn template_type(content: &NotificationContent) -> MailTemplateType {
    match content {
        NotificationContent::Watchlist {
            change: NotificationWatchlistChange::PriceChange { .. },
            ..
        } => MailTemplateType::WatchlistUpdatePrice,
        NotificationContent::Watchlist { .. } => MailTemplateType::WatchlistUpdateState,
        NotificationContent::SearchFilter { .. } => MailTemplateType::SearchFilterMatch,
        NotificationContent::PartnerApplication {
            decision: PartnerApplicationDecision::Approved,
            ..
        } => MailTemplateType::PartnerApplicationApproval,
        NotificationContent::PartnerApplication { .. } => {
            MailTemplateType::PartnerApplicationRejection
        }
    }
}

pub(crate) const fn template_directory(template_type: MailTemplateType) -> &'static str {
    match template_type {
        MailTemplateType::WatchlistUpdatePrice => "mjml/watchlist/product-update/price",
        MailTemplateType::WatchlistUpdateState => "mjml/watchlist/product-update/state",
        MailTemplateType::SearchFilterMatch => "mjml/search-filter/match",
        MailTemplateType::PartnerApplicationApproval => "mjml/partner-application/approval",
        MailTemplateType::PartnerApplicationRejection => "mjml/partner-application/rejection",
    }
}

pub(crate) fn s3_template_key(
    stage: &str,
    commit_sha: &str,
    template_type: MailTemplateType,
    language: Language,
) -> String {
    format!(
        "{stage}/{commit_sha}/{}/{}.html",
        template_directory(template_type),
        language.as_str()
    )
}

pub(crate) const fn ses_template_tag_value(template_type: MailTemplateType) -> &'static str {
    match template_type {
        MailTemplateType::WatchlistUpdatePrice => "WATCHLIST_UPDATE_PRICE",
        MailTemplateType::WatchlistUpdateState => "WATCHLIST_UPDATE_STATE",
        MailTemplateType::SearchFilterMatch => "SEARCH_FILTER_MATCH",
        MailTemplateType::PartnerApplicationApproval => "PARTNER_APPLICATION_APPROVAL",
        MailTemplateType::PartnerApplicationRejection => "PARTNER_APPLICATION_REJECTION",
    }
}

pub(crate) const fn subject(template_type: MailTemplateType) -> &'static str {
    match template_type {
        MailTemplateType::WatchlistUpdatePrice => "Your watchlist price changed",
        MailTemplateType::WatchlistUpdateState => "Your watchlist item changed",
        MailTemplateType::SearchFilterMatch => "New search filter match",
        MailTemplateType::PartnerApplicationApproval => "Partner application approved",
        MailTemplateType::PartnerApplicationRejection => "Partner application update",
    }
}

pub(crate) fn template_data(source: &NotificationDeliverySource) -> Value {
    let localized = source
        .content
        .clone()
        .localized(&source.currency, &[source.language]);

    match localized {
        LocalizedNotificationContent::Watchlist {
            snapshot, change, ..
        } => {
            let mut data = product_template_data(
                snapshot.shop_name.to_string(),
                snapshot.shop_slug_id.to_string(),
                snapshot.product_slug_id.to_string(),
                snapshot.title.map(|title| title.payload.to_string()),
                snapshot.image.map(|image| image.url.to_string()),
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
                    data["old_state"] = json!(old_state.format_human_readable(&Language::En));
                    data["new_state"] = json!(new_state.format_human_readable(&Language::En));
                    data["notification_type"] = json!("state_change");
                }
            }

            add_recipient_data(data, source.recipient_first_name.as_deref())
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
                snapshot.image.map(|image| image.url.to_string()),
                snapshot.view_url.to_string(),
            );
            data["search_filter_id"] = json!(user_search_filter_id.to_string());
            data["search_filter_name"] = json!(user_search_filter_name.to_string());
            data["notification_type"] = json!("search_filter_match");
            add_recipient_data(data, source.recipient_first_name.as_deref())
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
            source.recipient_first_name.as_deref(),
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
    json!({
        "shop_name": shop_name,
        "shop_slug_id": shop_slug_id,
        "product_slug_id": product_slug_id,
        "title": title,
        "image_url": image_url,
        "view_url": view_url,
    })
}

fn price_text(price: Option<Price>) -> Option<String> {
    price.map(|price| price.format_human_readable())
}

#[cfg(test)]
mod tests {
    use super::*;
    use common::{
        currency::domain::Currency, event_id::EventId, notification_id::NotificationId,
        product_id::ProductId, product_slug_id::ProductSlugId, shop_id::ShopId,
        shop_name::ShopName, shop_slug_id::ShopSlugId, shops_product_id::ShopsProductId,
        user_id::UserId,
    };
    use notification_core::notification::{
        NotificationContent, NotificationWatchlistChange, ProductNotificationSnapshot,
    };
    use notification_core::notification_delivery_id::NotificationDeliveryId;
    use rstest::rstest;
    use std::collections::HashMap;
    use url::Url;

    #[rstest]
    #[case(
        MailTemplateType::WatchlistUpdatePrice,
        "mjml/watchlist/product-update/price",
        "WATCHLIST_UPDATE_PRICE"
    )]
    #[case(
        MailTemplateType::WatchlistUpdateState,
        "mjml/watchlist/product-update/state",
        "WATCHLIST_UPDATE_STATE"
    )]
    #[case(
        MailTemplateType::SearchFilterMatch,
        "mjml/search-filter/match",
        "SEARCH_FILTER_MATCH"
    )]
    #[case(
        MailTemplateType::PartnerApplicationApproval,
        "mjml/partner-application/approval",
        "PARTNER_APPLICATION_APPROVAL"
    )]
    #[case(
        MailTemplateType::PartnerApplicationRejection,
        "mjml/partner-application/rejection",
        "PARTNER_APPLICATION_REJECTION"
    )]
    fn should_map_template_type_to_provider_values(
        #[case] template_type: MailTemplateType,
        #[case] directory: &str,
        #[case] tag_value: &str,
    ) {
        assert_eq!(template_directory(template_type), directory);
        assert_eq!(ses_template_tag_value(template_type), tag_value);
    }

    #[test]
    fn should_build_template_key_for_recipient_language() {
        assert_eq!(
            s3_template_key(
                "production",
                "abc123",
                MailTemplateType::WatchlistUpdatePrice,
                Language::De,
            ),
            "production/abc123/mjml/watchlist/product-update/price/de.html"
        );
    }

    #[test]
    fn should_localize_watchlist_prices_to_eur_and_english() -> Result<(), url::ParseError> {
        let content = NotificationContent::Watchlist {
            origin_event_id: EventId::new(),
            product_id: ProductId::new(),
            snapshot: ProductNotificationSnapshot {
                shop_id: ShopId::new(),
                shops_product_id: ShopsProductId::from("merchant-product"),
                shop_slug_id: ShopSlugId::from("aster-antiques"),
                product_slug_id: ProductSlugId::from("wooden-chair"),
                shop_name: ShopName::from("Aster Antiques"),
                title: None,
                image: None,
                url: Url::parse("https://merchant.example/products/wooden-chair")?,
                view_url: Url::parse("https://aura.example/products/wooden-chair")?,
            },
            change: NotificationWatchlistChange::PriceChange {
                old_price: HashMap::from([
                    (Currency::Usd, 1099_u64.into()),
                    (Currency::Eur, 999_u64.into()),
                ]),
                new_price: HashMap::from([
                    (Currency::Usd, 899_u64.into()),
                    (Currency::Eur, 799_u64.into()),
                ]),
            },
        };

        let data = template_data(&delivery_source(content, Currency::Eur, Language::En));

        assert_eq!(data["old_price"], json!("9,99 €"));
        assert_eq!(data["new_price"], json!("7,99 €"));
        assert_eq!(data["notification_type"], json!("price_change"));
        Ok(())
    }

    fn delivery_source(
        content: NotificationContent,
        currency: Currency,
        language: Language,
    ) -> NotificationDeliverySource {
        NotificationDeliverySource {
            notification_delivery_id: NotificationDeliveryId::new(),
            notification_id: NotificationId::new(),
            user_id: UserId::new(),
            content,
            recipient_email: "ada@example.test".to_owned(),
            recipient_first_name: Some("Ada".to_owned()),
            language,
            currency,
        }
    }
}

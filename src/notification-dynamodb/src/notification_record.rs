use crate::{
    currency_record::CurrencyRecord, language_record::LanguageRecord,
    notification_reason_record::NotificationReasonRecord,
    notification_type_record::NotificationTypeRecord, price_record::PriceRecord,
};
use common::partner_shop_application_id::PartnerShopApplicationId;
use common::user_search_filter_id::UserSearchFilterId;
use common::user_search_filter_name::UserSearchFilterName;
use common::{
    error::missing_field::MissingPersistenceField, event_id::EventId, product_id::ProductId,
    product_slug_id::ProductSlugId, product_state::domain::ProductState, shop_id::ShopId,
    shop_name::ShopName, shop_slug_id::ShopSlugId, shops_product_id::ShopsProductId,
    user_id::UserId,
};
use field::field;
use localization::Language;
use money::{Currency, MonetaryAmount, Price};
use notification_core::{
    notification::{
        Notification, NotificationPartnerApplicationPayload, NotificationPayload,
        NotificationSearchFilterPayload, NotificationWatchlistPayload, RehydratedNotificationState,
    },
    notification_id::NotificationId,
};
use product::dynamodb::{
    product_image_record::ProductImageRecord, product_state_record::ProductStateRecord,
    prohibited_content_record::ProhibitedContentRecord,
};
use product_core::{
    product_image::ProductImage, prohibited_content::ProhibitedContent, title::Title,
};

use serde::{Deserialize, Serialize};
use serde_fields::SerdeField;
use std::collections::HashMap;
use time::OffsetDateTime;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, SerdeField)]
pub(crate) struct NotificationRecord {
    pub pk: String,
    pub sk: String,
    pub lsi1_sk: String,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub lsi2_sk: Option<String>,
    pub user_id: UserId,
    pub origin_event_id: EventId,
    pub notification_id: NotificationId,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub notification_type: Option<NotificationTypeRecord>,
    pub notification_reason: NotificationReasonRecord,
    pub seen: bool,
    #[serde(default)]
    pub external: bool,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub image: Option<ProductImageRecord>,

    // watchlist
    // product
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub product_id: Option<ProductId>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub product_slug_id: Option<ProductSlugId>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub shop_slug_id: Option<ShopSlugId>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub shop_id: Option<ShopId>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub shops_product_id: Option<ShopsProductId>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub shop_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub title_de: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub title_en: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub title_fr: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub title_es: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub title_it: Option<String>,
    // price-change
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub new_price_native: Option<PriceRecord>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub new_price_eur: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub new_price_usd: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub new_price_gbp: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub new_price_aud: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub new_price_cad: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub new_price_nzd: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub new_price_cny: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub new_price_brl: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub new_price_pln: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub new_price_try: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub new_price_jpy: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub new_price_czk: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub new_price_rub: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub new_price_aed: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub new_price_sar: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub new_price_hkd: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub new_price_sgd: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub new_price_chf: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub old_price_native: Option<PriceRecord>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub old_price_eur: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub old_price_usd: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub old_price_gbp: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub old_price_aud: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub old_price_cad: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub old_price_nzd: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub old_price_cny: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub old_price_brl: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub old_price_pln: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub old_price_try: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub old_price_jpy: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub old_price_czk: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub old_price_rub: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub old_price_aed: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub old_price_sar: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub old_price_hkd: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub old_price_sgd: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub old_price_chf: Option<u64>,
    // state-change
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub new_state: Option<ProductStateRecord>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub old_state: Option<ProductStateRecord>,

    // search-filter
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub user_search_filter_id: Option<UserSearchFilterId>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub user_search_filter_name: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub url: Option<url::Url>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub view_url: Option<url::Url>,

    // partner-application
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub partner_application_id: Option<PartnerShopApplicationId>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub partner_application_image: Option<url::Url>,

    #[serde(with = "time::serde::rfc3339")]
    pub created: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub updated: OffsetDateTime,
    pub ttl: i64,
}

pub fn mk_pk(user_id: &UserId) -> String {
    format!("user#{user_id}")
}

pub fn mk_sk(origin_event_id: &EventId) -> String {
    format!("user#notification#origin_event_id#{origin_event_id}")
}

pub fn mk_lsi1_sk(
    notification_id: &NotificationId,
    notification_reason: &NotificationReasonRecord,
) -> String {
    let format_watchlist: fn(&NotificationId) -> String =
        |notification_id| format!("user#notification#watchlist#{notification_id}");
    match notification_reason {
        NotificationReasonRecord::WatchlistStateChanged => format_watchlist(notification_id),
        NotificationReasonRecord::WatchlistPriceChanged => format_watchlist(notification_id),
        NotificationReasonRecord::SearchFilterMatch => {
            format!("user#notification#search_filter#{notification_id}")
        }
        NotificationReasonRecord::PartnerApplicationApproved => {
            format!("user#notification#partner_application#{notification_id}")
        }
        NotificationReasonRecord::PartnerApplicationRejected => {
            format!("user#notification#partner_application#{notification_id}")
        }
    }
}

pub fn mk_lsi2_sk(product_id: &ProductId, origin_event_id: &EventId) -> String {
    format!("user#notification#product_id#{product_id}#origin_event_id#{origin_event_id}")
}

const LSI2_SK_LOWER_BOUND_PREFIX: &str = "user#notification#product_id#";

pub fn mk_lsi2_sk_product_prefix(product_id: &ProductId) -> (String, String) {
    let prefix = format!("{LSI2_SK_LOWER_BOUND_PREFIX}{product_id}#origin_event_id#");
    let upper = format!("{prefix}\u{ffff}");
    (prefix, upper)
}

fn derive_notification_reason(
    watchlist_payload: &NotificationWatchlistPayload,
) -> NotificationReasonRecord {
    match watchlist_payload {
        NotificationWatchlistPayload::PriceChange { .. } => {
            NotificationReasonRecord::WatchlistPriceChanged
        }
        NotificationWatchlistPayload::StateChange { .. } => {
            NotificationReasonRecord::WatchlistStateChanged
        }
    }
}

fn extract_currency_amount(
    prices: &HashMap<Currency, MonetaryAmount>,
    currency: CurrencyRecord,
) -> Option<u64> {
    prices.get(&currency.into()).map(|amount| (*amount).into())
}

fn title_for_language(
    title: Option<&HashMap<Language, Title>>,
    language: LanguageRecord,
) -> Option<String> {
    title
        .and_then(|title| title.get(&language.into()))
        .map(|title| String::from(title.clone()))
}

fn build_title(record: &NotificationRecord) -> Option<HashMap<Language, Title>> {
    let title: HashMap<Language, Title> = [
        (LanguageRecord::De, record.title_de.as_ref()),
        (LanguageRecord::En, record.title_en.as_ref()),
        (LanguageRecord::Fr, record.title_fr.as_ref()),
        (LanguageRecord::Es, record.title_es.as_ref()),
        (LanguageRecord::It, record.title_it.as_ref()),
    ]
    .into_iter()
    .filter_map(|(language, title)| {
        title.map(|title| (language.into(), Title::from(title.clone())))
    })
    .collect();

    (!title.is_empty()).then_some(title)
}

fn compute_ttl(created: &OffsetDateTime) -> i64 {
    (*created + time::Duration::days(7)).unix_timestamp()
}

fn product_image_record_from_domain(value: ProductImage) -> ProductImageRecord {
    ProductImageRecord {
        url: value.url,
        prohibited_content: match value.prohibited_content {
            ProhibitedContent::Unknown => ProhibitedContentRecord::Unknown,
            ProhibitedContent::None => ProhibitedContentRecord::None,
            ProhibitedContent::NaziGermany => ProhibitedContentRecord::NaziGermany,
        },
    }
}

fn product_image_from_record(value: ProductImageRecord) -> ProductImage {
    ProductImage {
        url: value.url,
        prohibited_content: match value.prohibited_content {
            ProhibitedContentRecord::Unknown => ProhibitedContent::Unknown,
            ProhibitedContentRecord::None => ProhibitedContent::None,
            ProhibitedContentRecord::NaziGermany => ProhibitedContent::NaziGermany,
        },
    }
}

impl NotificationRecord {
    pub(crate) fn from_notification(notification: &Notification) -> Self {
        let now = OffsetDateTime::now_utc();
        match notification.notification_payload().clone() {
            NotificationPayload::Watchlist {
                product_id,
                shop_id,
                shops_product_id,
                shop_slug_id,
                product_slug_id,
                shop_name,
                title,
                image,
                url,
                view_url,
                watchlist_payload,
            } => {
                let notification_reason = derive_notification_reason(&watchlist_payload);
                let lsi1_sk = mk_lsi1_sk(&notification.notification_id(), &notification_reason);

                let (
                    new_price_native,
                    new_price_eur,
                    new_price_usd,
                    new_price_gbp,
                    new_price_aud,
                    new_price_cad,
                    new_price_nzd,
                    new_price_cny,
                    new_price_brl,
                    new_price_pln,
                    new_price_try,
                    new_price_jpy,
                    new_price_czk,
                    new_price_rub,
                    new_price_aed,
                    new_price_sar,
                    new_price_hkd,
                    new_price_sgd,
                    new_price_chf,
                    old_price_native,
                    old_price_eur,
                    old_price_usd,
                    old_price_gbp,
                    old_price_aud,
                    old_price_cad,
                    old_price_nzd,
                    old_price_cny,
                    old_price_brl,
                    old_price_pln,
                    old_price_try,
                    old_price_jpy,
                    old_price_czk,
                    old_price_rub,
                    old_price_aed,
                    old_price_sar,
                    old_price_hkd,
                    old_price_sgd,
                    old_price_chf,
                    new_state,
                    old_state,
                ) = match &watchlist_payload {
                    NotificationWatchlistPayload::PriceChange {
                        old_price,
                        new_price,
                    } => {
                        let new_native = new_price.iter().next().map(|(c, a)| {
                            PriceRecord::from(Price {
                                monetary_amount: *a,
                                currency: *c,
                            })
                        });
                        let old_native = old_price.iter().next().map(|(c, a)| {
                            PriceRecord::from(Price {
                                monetary_amount: *a,
                                currency: *c,
                            })
                        });
                        (
                            new_native,
                            extract_currency_amount(new_price, CurrencyRecord::Eur),
                            extract_currency_amount(new_price, CurrencyRecord::Usd),
                            extract_currency_amount(new_price, CurrencyRecord::Gbp),
                            extract_currency_amount(new_price, CurrencyRecord::Aud),
                            extract_currency_amount(new_price, CurrencyRecord::Cad),
                            extract_currency_amount(new_price, CurrencyRecord::Nzd),
                            extract_currency_amount(new_price, CurrencyRecord::Cny),
                            extract_currency_amount(new_price, CurrencyRecord::Brl),
                            extract_currency_amount(new_price, CurrencyRecord::Pln),
                            extract_currency_amount(new_price, CurrencyRecord::Try),
                            extract_currency_amount(new_price, CurrencyRecord::Jpy),
                            extract_currency_amount(new_price, CurrencyRecord::Czk),
                            extract_currency_amount(new_price, CurrencyRecord::Rub),
                            extract_currency_amount(new_price, CurrencyRecord::Aed),
                            extract_currency_amount(new_price, CurrencyRecord::Sar),
                            extract_currency_amount(new_price, CurrencyRecord::Hkd),
                            extract_currency_amount(new_price, CurrencyRecord::Sgd),
                            extract_currency_amount(new_price, CurrencyRecord::Chf),
                            old_native,
                            extract_currency_amount(old_price, CurrencyRecord::Eur),
                            extract_currency_amount(old_price, CurrencyRecord::Usd),
                            extract_currency_amount(old_price, CurrencyRecord::Gbp),
                            extract_currency_amount(old_price, CurrencyRecord::Aud),
                            extract_currency_amount(old_price, CurrencyRecord::Cad),
                            extract_currency_amount(old_price, CurrencyRecord::Nzd),
                            extract_currency_amount(old_price, CurrencyRecord::Cny),
                            extract_currency_amount(old_price, CurrencyRecord::Brl),
                            extract_currency_amount(old_price, CurrencyRecord::Pln),
                            extract_currency_amount(old_price, CurrencyRecord::Try),
                            extract_currency_amount(old_price, CurrencyRecord::Jpy),
                            extract_currency_amount(old_price, CurrencyRecord::Czk),
                            extract_currency_amount(old_price, CurrencyRecord::Rub),
                            extract_currency_amount(old_price, CurrencyRecord::Aed),
                            extract_currency_amount(old_price, CurrencyRecord::Sar),
                            extract_currency_amount(old_price, CurrencyRecord::Hkd),
                            extract_currency_amount(old_price, CurrencyRecord::Sgd),
                            extract_currency_amount(old_price, CurrencyRecord::Chf),
                            None,
                            None,
                        )
                    }
                    NotificationWatchlistPayload::StateChange {
                        old_state,
                        new_state,
                    } => (
                        None,
                        None,
                        None,
                        None,
                        None,
                        None,
                        None,
                        None,
                        None,
                        None,
                        None,
                        None,
                        None,
                        None,
                        None,
                        None,
                        None,
                        None,
                        None,
                        None,
                        None,
                        None,
                        None,
                        None,
                        None,
                        None,
                        None,
                        None,
                        None,
                        None,
                        None,
                        None,
                        None,
                        None,
                        None,
                        None,
                        None,
                        None,
                        Some(ProductStateRecord::from(*new_state)),
                        Some(ProductStateRecord::from(*old_state)),
                    ),
                };

                let lsi2_sk = mk_lsi2_sk(&product_id, &notification.origin_event_id());

                NotificationRecord {
                    pk: mk_pk(&notification.user_id()),
                    sk: mk_sk(&notification.origin_event_id()),
                    lsi1_sk,
                    lsi2_sk: Some(lsi2_sk),
                    user_id: notification.user_id(),
                    origin_event_id: notification.origin_event_id(),
                    notification_id: notification.notification_id(),
                    notification_type: notification.notification_type().map(Into::into),
                    notification_reason,
                    seen: notification.seen(),
                    external: notification.external(),
                    image: image.map(product_image_record_from_domain),
                    product_id: Some(product_id),
                    product_slug_id: Some(product_slug_id),
                    shop_slug_id: Some(shop_slug_id),
                    shop_id: Some(shop_id),
                    shops_product_id: Some(shops_product_id),
                    shop_name: Some(String::from(shop_name)),
                    title_de: title_for_language(title.as_ref(), LanguageRecord::De),
                    title_en: title_for_language(title.as_ref(), LanguageRecord::En),
                    title_fr: title_for_language(title.as_ref(), LanguageRecord::Fr),
                    title_es: title_for_language(title.as_ref(), LanguageRecord::Es),
                    title_it: title_for_language(title.as_ref(), LanguageRecord::It),
                    user_search_filter_id: None,
                    user_search_filter_name: None,
                    url: Some(url),
                    view_url: Some(view_url),
                    partner_application_id: None,
                    partner_application_image: None,
                    new_price_native,
                    new_price_eur,
                    new_price_usd,
                    new_price_gbp,
                    new_price_aud,
                    new_price_cad,
                    new_price_nzd,
                    new_price_cny,
                    new_price_brl,
                    new_price_pln,
                    new_price_try,
                    new_price_jpy,
                    new_price_czk,
                    new_price_rub,
                    new_price_aed,
                    new_price_sar,
                    new_price_hkd,
                    new_price_sgd,
                    new_price_chf,
                    old_price_native,
                    old_price_eur,
                    old_price_usd,
                    old_price_gbp,
                    old_price_aud,
                    old_price_cad,
                    old_price_nzd,
                    old_price_cny,
                    old_price_brl,
                    old_price_pln,
                    old_price_try,
                    old_price_jpy,
                    old_price_czk,
                    old_price_rub,
                    old_price_aed,
                    old_price_sar,
                    old_price_hkd,
                    old_price_sgd,
                    old_price_chf,
                    new_state,
                    old_state,
                    created: now,
                    updated: now,
                    ttl: compute_ttl(&now),
                }
            }
            NotificationPayload::SearchFilter {
                product_id,
                shop_id,
                shops_product_id,
                shop_slug_id,
                product_slug_id,
                shop_name,
                title,
                image,
                url,
                view_url,
                search_filter_payload,
            } => {
                let notification_reason = NotificationReasonRecord::SearchFilterMatch;
                let lsi1_sk = mk_lsi1_sk(&notification.notification_id(), &notification_reason);
                let lsi2_sk = mk_lsi2_sk(&product_id, &notification.origin_event_id());
                NotificationRecord {
                    pk: mk_pk(&notification.user_id()),
                    sk: mk_sk(&notification.origin_event_id()),
                    lsi1_sk,
                    lsi2_sk: Some(lsi2_sk),
                    user_id: notification.user_id(),
                    origin_event_id: notification.origin_event_id(),
                    notification_id: notification.notification_id(),
                    notification_type: notification.notification_type().map(Into::into),
                    notification_reason,
                    seen: notification.seen(),
                    external: notification.external(),
                    image: image.map(product_image_record_from_domain),
                    product_id: Some(product_id),
                    product_slug_id: Some(product_slug_id),
                    shop_slug_id: Some(shop_slug_id),
                    shop_id: Some(shop_id),
                    shops_product_id: Some(shops_product_id),
                    shop_name: Some(String::from(shop_name)),
                    title_de: title_for_language(title.as_ref(), LanguageRecord::De),
                    title_en: title_for_language(title.as_ref(), LanguageRecord::En),
                    title_fr: title_for_language(title.as_ref(), LanguageRecord::Fr),
                    title_es: title_for_language(title.as_ref(), LanguageRecord::Es),
                    title_it: title_for_language(title.as_ref(), LanguageRecord::It),
                    user_search_filter_id: Some(search_filter_payload.user_search_filter_id),
                    user_search_filter_name: Some(String::from(
                        search_filter_payload.user_search_filter_name,
                    )),
                    url: Some(url),
                    view_url: Some(view_url),
                    partner_application_id: None,
                    partner_application_image: None,
                    new_price_native: None,
                    new_price_eur: None,
                    new_price_usd: None,
                    new_price_gbp: None,
                    new_price_aud: None,
                    new_price_cad: None,
                    new_price_nzd: None,
                    new_price_cny: None,
                    new_price_brl: None,
                    new_price_pln: None,
                    new_price_try: None,
                    new_price_jpy: None,
                    new_price_czk: None,
                    new_price_rub: None,
                    new_price_aed: None,
                    new_price_sar: None,
                    new_price_hkd: None,
                    new_price_sgd: None,
                    new_price_chf: None,
                    old_price_native: None,
                    old_price_eur: None,
                    old_price_usd: None,
                    old_price_gbp: None,
                    old_price_aud: None,
                    old_price_cad: None,
                    old_price_nzd: None,
                    old_price_cny: None,
                    old_price_brl: None,
                    old_price_pln: None,
                    old_price_try: None,
                    old_price_jpy: None,
                    old_price_czk: None,
                    old_price_rub: None,
                    old_price_aed: None,
                    old_price_sar: None,
                    old_price_hkd: None,
                    old_price_sgd: None,
                    old_price_chf: None,
                    new_state: None,
                    old_state: None,
                    created: now,
                    updated: now,
                    ttl: compute_ttl(&now),
                }
            }
            NotificationPayload::PartnerApplication {
                shop_name,
                image,
                partner_application_payload,
            } => {
                let (notification_reason, partner_application_id) =
                    match partner_application_payload {
                        NotificationPartnerApplicationPayload::Approved {
                            partner_application_id,
                        } => (
                            NotificationReasonRecord::PartnerApplicationApproved,
                            partner_application_id,
                        ),
                        NotificationPartnerApplicationPayload::Rejected {
                            partner_application_id,
                        } => (
                            NotificationReasonRecord::PartnerApplicationRejected,
                            partner_application_id,
                        ),
                    };
                let lsi1_sk = mk_lsi1_sk(&notification.notification_id(), &notification_reason);
                NotificationRecord {
                    pk: mk_pk(&notification.user_id()),
                    sk: mk_sk(&notification.origin_event_id()),
                    lsi1_sk,
                    lsi2_sk: None,
                    user_id: notification.user_id(),
                    origin_event_id: notification.origin_event_id(),
                    notification_id: notification.notification_id(),
                    notification_type: notification.notification_type().map(Into::into),
                    notification_reason,
                    seen: notification.seen(),
                    external: notification.external(),
                    image: None,
                    product_id: None,
                    product_slug_id: None,
                    shop_slug_id: None,
                    shop_id: None,
                    shops_product_id: None,
                    shop_name: Some(String::from(shop_name)),
                    title_de: None,
                    title_en: None,
                    title_fr: None,
                    title_es: None,
                    title_it: None,
                    user_search_filter_id: None,
                    user_search_filter_name: None,
                    url: None,
                    view_url: None,
                    partner_application_id: Some(partner_application_id),
                    partner_application_image: image,
                    new_price_native: None,
                    new_price_eur: None,
                    new_price_usd: None,
                    new_price_gbp: None,
                    new_price_aud: None,
                    new_price_cad: None,
                    new_price_nzd: None,
                    new_price_cny: None,
                    new_price_brl: None,
                    new_price_pln: None,
                    new_price_try: None,
                    new_price_jpy: None,
                    new_price_czk: None,
                    new_price_rub: None,
                    new_price_aed: None,
                    new_price_sar: None,
                    new_price_hkd: None,
                    new_price_sgd: None,
                    new_price_chf: None,
                    old_price_native: None,
                    old_price_eur: None,
                    old_price_usd: None,
                    old_price_gbp: None,
                    old_price_aud: None,
                    old_price_cad: None,
                    old_price_nzd: None,
                    old_price_cny: None,
                    old_price_brl: None,
                    old_price_pln: None,
                    old_price_try: None,
                    old_price_jpy: None,
                    old_price_czk: None,
                    old_price_rub: None,
                    old_price_aed: None,
                    old_price_sar: None,
                    old_price_hkd: None,
                    old_price_sgd: None,
                    old_price_chf: None,
                    new_state: None,
                    old_state: None,
                    created: now,
                    updated: now,
                    ttl: compute_ttl(&now),
                }
            }
        }
    }
}

fn build_price_map(
    native: Option<PriceRecord>,
    currency_amounts: &[(CurrencyRecord, Option<u64>)],
) -> HashMap<Currency, MonetaryAmount> {
    let mut map = HashMap::new();
    if let Some(native) = native {
        let price: Price = native.into();
        map.insert(price.currency, price.monetary_amount);
    }
    for &(currency, amount) in currency_amounts {
        if let Some(amount) = amount {
            map.insert(currency.into(), amount.into());
        }
    }
    map
}

impl TryFrom<NotificationRecord> for Notification {
    type Error = MissingPersistenceField;

    fn try_from(record: NotificationRecord) -> Result<Self, Self::Error> {
        let notification_payload = if record.notification_reason.is_partner_application() {
            let shop_name = record.shop_name.map(ShopName::from).ok_or_else(|| {
                MissingPersistenceField::new(field!(shop_name@NotificationRecord))
            })?;
            let partner_application_id = record.partner_application_id.ok_or_else(|| {
                MissingPersistenceField::new(field!(partner_application_id@NotificationRecord))
            })?;
            let partner_application_payload = match record.notification_reason {
                NotificationReasonRecord::PartnerApplicationApproved => {
                    NotificationPartnerApplicationPayload::Approved {
                        partner_application_id,
                    }
                }
                _ => NotificationPartnerApplicationPayload::Rejected {
                    partner_application_id,
                },
            };
            NotificationPayload::PartnerApplication {
                shop_name,
                image: record.partner_application_image,
                partner_application_payload,
            }
        } else {
            let title = build_title(&record);

            let product_id = record.product_id.ok_or_else(|| {
                MissingPersistenceField::new(field!(product_id@NotificationRecord))
            })?;
            let shop_id = record
                .shop_id
                .ok_or_else(|| MissingPersistenceField::new(field!(shop_id@NotificationRecord)))?;
            let shops_product_id = record.shops_product_id.ok_or_else(|| {
                MissingPersistenceField::new(field!(shops_product_id@NotificationRecord))
            })?;
            let shop_slug_id = record.shop_slug_id.ok_or_else(|| {
                MissingPersistenceField::new(field!(shop_slug_id@NotificationRecord))
            })?;
            let product_slug_id = record.product_slug_id.ok_or_else(|| {
                MissingPersistenceField::new(field!(product_slug_id@NotificationRecord))
            })?;
            let shop_name = record.shop_name.map(ShopName::from).ok_or_else(|| {
                MissingPersistenceField::new(field!(shop_name@NotificationRecord))
            })?;

            let image = record.image.map(product_image_from_record);
            let url = record
                .url
                .ok_or_else(|| MissingPersistenceField::new(field!(url@NotificationRecord)))?;
            let view_url = record
                .view_url
                .ok_or_else(|| MissingPersistenceField::new(field!(view_url@NotificationRecord)))?;

            if record.notification_reason.is_search_filter() {
                let user_search_filter_id = record.user_search_filter_id.ok_or_else(|| {
                    MissingPersistenceField::new(field!(user_search_filter_id@NotificationRecord))
                })?;
                let user_search_filter_name = record
                    .user_search_filter_name
                    .map(UserSearchFilterName::from)
                    .ok_or_else(|| {
                        MissingPersistenceField::new(
                            field!(user_search_filter_name@NotificationRecord),
                        )
                    })?;

                NotificationPayload::SearchFilter {
                    product_id,
                    shop_id,
                    shops_product_id,
                    shop_slug_id,
                    product_slug_id,
                    shop_name,
                    title,
                    image,
                    url,
                    view_url,
                    search_filter_payload: NotificationSearchFilterPayload {
                        user_search_filter_id,
                        user_search_filter_name,
                    },
                }
            } else {
                let is_state_change = matches!(
                    record.notification_reason,
                    NotificationReasonRecord::WatchlistStateChanged
                );

                let watchlist_payload = if is_state_change {
                    let old_state = record.old_state.map(ProductState::from).ok_or_else(|| {
                        MissingPersistenceField::new(field!(old_state@NotificationRecord))
                    })?;
                    let new_state = record.new_state.map(ProductState::from).ok_or_else(|| {
                        MissingPersistenceField::new(field!(new_state@NotificationRecord))
                    })?;
                    NotificationWatchlistPayload::StateChange {
                        old_state,
                        new_state,
                    }
                } else {
                    NotificationWatchlistPayload::PriceChange {
                        old_price: build_price_map(
                            record.old_price_native,
                            &[
                                (CurrencyRecord::Eur, record.old_price_eur),
                                (CurrencyRecord::Usd, record.old_price_usd),
                                (CurrencyRecord::Gbp, record.old_price_gbp),
                                (CurrencyRecord::Aud, record.old_price_aud),
                                (CurrencyRecord::Cad, record.old_price_cad),
                                (CurrencyRecord::Nzd, record.old_price_nzd),
                                (CurrencyRecord::Cny, record.old_price_cny),
                                (CurrencyRecord::Brl, record.old_price_brl),
                                (CurrencyRecord::Pln, record.old_price_pln),
                                (CurrencyRecord::Try, record.old_price_try),
                                (CurrencyRecord::Jpy, record.old_price_jpy),
                                (CurrencyRecord::Czk, record.old_price_czk),
                                (CurrencyRecord::Rub, record.old_price_rub),
                                (CurrencyRecord::Aed, record.old_price_aed),
                                (CurrencyRecord::Sar, record.old_price_sar),
                                (CurrencyRecord::Hkd, record.old_price_hkd),
                                (CurrencyRecord::Sgd, record.old_price_sgd),
                                (CurrencyRecord::Chf, record.old_price_chf),
                            ],
                        ),
                        new_price: build_price_map(
                            record.new_price_native,
                            &[
                                (CurrencyRecord::Eur, record.new_price_eur),
                                (CurrencyRecord::Usd, record.new_price_usd),
                                (CurrencyRecord::Gbp, record.new_price_gbp),
                                (CurrencyRecord::Aud, record.new_price_aud),
                                (CurrencyRecord::Cad, record.new_price_cad),
                                (CurrencyRecord::Nzd, record.new_price_nzd),
                                (CurrencyRecord::Cny, record.new_price_cny),
                                (CurrencyRecord::Brl, record.new_price_brl),
                                (CurrencyRecord::Pln, record.new_price_pln),
                                (CurrencyRecord::Try, record.new_price_try),
                                (CurrencyRecord::Jpy, record.new_price_jpy),
                                (CurrencyRecord::Czk, record.new_price_czk),
                                (CurrencyRecord::Rub, record.new_price_rub),
                                (CurrencyRecord::Aed, record.new_price_aed),
                                (CurrencyRecord::Sar, record.new_price_sar),
                                (CurrencyRecord::Hkd, record.new_price_hkd),
                                (CurrencyRecord::Sgd, record.new_price_sgd),
                                (CurrencyRecord::Chf, record.new_price_chf),
                            ],
                        ),
                    }
                };

                NotificationPayload::Watchlist {
                    product_id,
                    shop_id,
                    shops_product_id,
                    shop_slug_id,
                    product_slug_id,
                    shop_name,
                    title,
                    image,
                    url,
                    view_url,
                    watchlist_payload,
                }
            }
        };

        Ok(Notification::rehydrate(RehydratedNotificationState {
            user_id: record.user_id,
            origin_event_id: record.origin_event_id,
            notification_id: record.notification_id,
            notification_type: record.notification_type.map(Into::into),
            notification_payload,
            seen: record.seen,
            external: record.external,
        }))
    }
}

#[cfg(feature = "test-data")]
mod faker {
    use super::*;
    use fake::{Dummy, Fake, Faker, RngExt};

    impl Dummy<Faker> for NotificationRecord {
        fn dummy_with_rng<R: RngExt + ?Sized>(config: &Faker, rng: &mut R) -> Self {
            let user_id: UserId = config.fake_with_rng(rng);
            let origin_event_id: EventId = config.fake_with_rng(rng);
            let notification_id = NotificationId::new();
            let notification_reason = NotificationReasonRecord::WatchlistPriceChanged;
            let created = OffsetDateTime::now_utc();
            let product_id: ProductId = config.fake_with_rng(rng);
            let shop_name: String = config.fake_with_rng(rng);
            let shop_slug_id = ShopSlugId::from(shop_name.as_str());
            let title_en: String = config.fake_with_rng(rng);
            let product_slug_id = ProductSlugId::from(title_en.as_str());

            NotificationRecord {
                pk: mk_pk(&user_id),
                sk: mk_sk(&origin_event_id),
                lsi1_sk: mk_lsi1_sk(&notification_id, &notification_reason),
                lsi2_sk: Some(mk_lsi2_sk(&product_id, &origin_event_id)),
                user_id,
                origin_event_id,
                notification_id,
                notification_type: config.fake_with_rng(rng),
                notification_reason,
                seen: config.fake_with_rng(rng),
                external: config.fake_with_rng(rng),
                image: None,
                product_id: Some(product_id),
                product_slug_id: Some(product_slug_id),
                shop_slug_id: Some(shop_slug_id),
                shop_id: Some(config.fake_with_rng(rng)),
                shops_product_id: Some(ShopsProductId::from("test-product-123")),
                shop_name: Some(shop_name),
                title_de: Some(config.fake_with_rng(rng)),
                title_en: Some(title_en),
                title_fr: None,
                title_es: None,
                title_it: None,
                new_price_native: None,
                new_price_eur: Some(config.fake_with_rng(rng)),
                new_price_usd: None,
                new_price_gbp: None,
                new_price_aud: None,
                new_price_cad: None,
                new_price_nzd: None,
                new_price_cny: None,
                new_price_brl: None,
                new_price_pln: None,
                new_price_try: None,
                new_price_jpy: None,
                new_price_czk: None,
                new_price_rub: None,
                new_price_aed: None,
                new_price_sar: None,
                new_price_hkd: None,
                new_price_sgd: None,
                new_price_chf: None,
                old_price_native: None,
                old_price_eur: Some(config.fake_with_rng(rng)),
                old_price_usd: None,
                old_price_gbp: None,
                old_price_aud: None,
                old_price_cad: None,
                old_price_nzd: None,
                old_price_cny: None,
                old_price_brl: None,
                old_price_pln: None,
                old_price_try: None,
                old_price_jpy: None,
                old_price_czk: None,
                old_price_rub: None,
                old_price_aed: None,
                old_price_sar: None,
                old_price_hkd: None,
                old_price_sgd: None,
                old_price_chf: None,
                new_state: Some(config.fake_with_rng(rng)),
                old_state: Some(config.fake_with_rng(rng)),
                user_search_filter_id: None,
                user_search_filter_name: None,
                url: Some(config.fake_with_rng(rng)),
                view_url: Some(config.fake_with_rng(rng)),
                partner_application_id: None,
                partner_application_image: None,
                created,
                updated: created,
                ttl: compute_ttl(&created),
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use fake::{Fake, Faker};

        #[test]
        fn should_fake_notification_record() {
            let _ = Faker.fake::<NotificationRecord>();
        }

        #[test]
        fn should_fake_notification_record_with_lsi2_sk() {
            let record = Faker.fake::<NotificationRecord>();
            assert!(record.lsi2_sk.is_some());
            let lsi2_sk = record.lsi2_sk.unwrap();
            assert!(lsi2_sk.starts_with("user#notification#product_id#"));
            assert!(lsi2_sk.contains("#origin_event_id#"));
        }
    }
}

#[cfg(test)]
mod key_tests {
    use super::*;
    use common::{event_id::EventId, product_id::ProductId};

    #[test]
    fn should_format_lsi2_sk_correctly() {
        let product_id = ProductId::new();
        let event_id = EventId::new();
        let lsi2_sk = mk_lsi2_sk(&product_id, &event_id);
        assert_eq!(
            lsi2_sk,
            format!("user#notification#product_id#{product_id}#origin_event_id#{event_id}")
        );
    }

    #[test]
    fn should_produce_prefix_bounds_for_product() {
        let product_id = ProductId::new();
        let (lower, upper) = mk_lsi2_sk_product_prefix(&product_id);
        assert!(lower.starts_with(&format!(
            "user#notification#product_id#{product_id}#origin_event_id#"
        )));
        assert!(upper.starts_with(&format!(
            "user#notification#product_id#{product_id}#origin_event_id#"
        )));
        assert!(lower < upper);
    }
}

#[cfg(all(test, feature = "test-data"))]
mod image_round_trip_tests {
    use super::*;
    use fake::{Fake, Faker};
    use product_core::product_image::ProductImage;
    use url::Url;

    fn extract_image(notification: &Notification) -> Option<ProductImage> {
        match notification.notification_payload() {
            NotificationPayload::Watchlist { image, .. } => image.clone(),
            NotificationPayload::SearchFilter { image, .. } => image.clone(),
            NotificationPayload::PartnerApplication { .. } => None,
        }
    }

    #[test]
    fn should_preserve_image_when_converting_notification_to_record_and_back() {
        let image: ProductImage = Faker.fake();
        let mut record = Faker.fake::<NotificationRecord>();
        record.image = Some(product_image_record_from_domain(image.clone()));

        let notification: Notification = record.try_into().expect("conversion should succeed");

        assert_eq!(
            Some(image),
            extract_image(&notification),
            "image should be preserved in round-trip"
        );
    }

    #[test]
    fn should_have_no_image_when_record_has_no_image() {
        let mut record = Faker.fake::<NotificationRecord>();
        record.image = None;

        let notification: Notification = record.try_into().expect("conversion should succeed");

        assert!(
            extract_image(&notification).is_none(),
            "image should be None when record has no image"
        );
    }

    fn extract_partner_application_image(notification: &Notification) -> Option<Url> {
        match notification.notification_payload() {
            NotificationPayload::PartnerApplication { image, .. } => image.clone(),
            _ => None,
        }
    }

    #[test]
    fn should_preserve_partner_application_image_when_converting_notification_to_record_and_back() {
        let image = Url::parse("https://example.com/logo.png").unwrap();
        let mut record = Faker.fake::<NotificationRecord>();
        record.notification_reason = NotificationReasonRecord::PartnerApplicationApproved;
        record.partner_application_id = Some(PartnerShopApplicationId::new());
        record.partner_application_image = Some(image.clone());
        record.image = None;

        let notification: Notification = record.try_into().expect("conversion should succeed");

        assert_eq!(
            Some(image),
            extract_partner_application_image(&notification),
            "partner application image should be preserved in round-trip"
        );
    }
}

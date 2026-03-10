use crate::{
    core::{
        notification::{Notification, NotificationPayload, NotificationWatchlistPayload},
        notification_id::NotificationId,
    },
    dynamodb::notification_reason_record::NotificationReasonRecord,
};
use common::{
    currency::domain::Currency,
    language::domain::Language,
    price::domain::{MonetaryAmount, Price},
    price::record::PriceRecord,
    product_id::ProductId,
    product_state::domain::ProductState,
    shop_id::ShopId,
    shop_name::ShopName,
    shops_product_id::ShopsProductId,
    slug_id::SlugId,
    user_id::UserId,
};
use product::core::title::Title;
use product::dynamodb::{
    product_image_record::ProductImageRecord, product_state_record::ProductStateRecord,
};
use serde::{Deserialize, Serialize};
use serde_fields::SerdeField;
use std::collections::HashMap;
use time::OffsetDateTime;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, SerdeField)]
pub struct NotificationRecord {
    pub pk: String,
    pub sk: String,
    pub lsi1_sk: String,
    pub user_id: UserId,
    pub notification_id: NotificationId,
    pub notification_reason: NotificationReasonRecord,
    pub seen: bool,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub image: Option<ProductImageRecord>,

    // watchlist
    // product
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub product_id: Option<ProductId>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub product_slug_id: Option<SlugId<6>>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub shop_slug_id: Option<SlugId<0>>,
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
    // state-change
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub new_state: Option<ProductStateRecord>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub old_state: Option<ProductStateRecord>,

    #[serde(with = "time::serde::rfc3339")]
    pub created: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub updated: OffsetDateTime,
    pub ttl: i64,
}

pub fn mk_pk(user_id: &UserId) -> String {
    format!("user#{user_id}")
}

pub fn mk_sk(notification_id: &NotificationId) -> String {
    format!("user#notification#{notification_id}")
}

pub fn mk_lsi1_sk(
    notification_id: &NotificationId,
    notification_reason: &NotificationReasonRecord,
) -> String {
    let format_watchlist: fn(&NotificationId) -> String =
        |notification_id| format!("user#notification#watchlist#{notification_id}");
    match notification_reason {
        NotificationReasonRecord::WatchlistStateListed => format_watchlist(notification_id),
        NotificationReasonRecord::WatchlistStateAvailable => format_watchlist(notification_id),
        NotificationReasonRecord::WatchlistStateReserved => format_watchlist(notification_id),
        NotificationReasonRecord::WatchlistStateSold => format_watchlist(notification_id),
        NotificationReasonRecord::WatchlistStateRemoved => format_watchlist(notification_id),
        NotificationReasonRecord::WatchlistStateUnknown => format_watchlist(notification_id),
        NotificationReasonRecord::WatchlistPriceDiscovered => format_watchlist(notification_id),
        NotificationReasonRecord::WatchlistPriceDropped => format_watchlist(notification_id),
        NotificationReasonRecord::WatchlistPriceIncreased => format_watchlist(notification_id),
        NotificationReasonRecord::WatchlistPriceRemoved => format_watchlist(notification_id),
    }
}

fn derive_notification_reason(
    watchlist_payload: &NotificationWatchlistPayload,
) -> NotificationReasonRecord {
    match watchlist_payload {
        NotificationWatchlistPayload::PriceChange {
            old_price,
            new_price,
        } => {
            let old_eur = old_price.get(&Currency::Eur).copied();
            let new_eur = new_price.get(&Currency::Eur).copied();
            match (old_eur, new_eur) {
                (None, Some(_)) => NotificationReasonRecord::WatchlistPriceDiscovered,
                (Some(_), None) => NotificationReasonRecord::WatchlistPriceRemoved,
                (Some(old), Some(new)) => {
                    let old_val: u64 = old.into();
                    let new_val: u64 = new.into();
                    if new_val < old_val {
                        NotificationReasonRecord::WatchlistPriceDropped
                    } else {
                        NotificationReasonRecord::WatchlistPriceIncreased
                    }
                }
                (None, None) => NotificationReasonRecord::WatchlistPriceDiscovered,
            }
        }
        NotificationWatchlistPayload::StateChange { new_state, .. } => match new_state {
            ProductState::Listed => NotificationReasonRecord::WatchlistStateListed,
            ProductState::Available => NotificationReasonRecord::WatchlistStateAvailable,
            ProductState::Reserved => NotificationReasonRecord::WatchlistStateReserved,
            ProductState::Sold => NotificationReasonRecord::WatchlistStateSold,
            ProductState::Removed => NotificationReasonRecord::WatchlistStateRemoved,
            ProductState::Unknown => NotificationReasonRecord::WatchlistStateUnknown,
        },
    }
}

fn extract_currency_amount(
    prices: &HashMap<Currency, MonetaryAmount>,
    currency: &Currency,
) -> Option<u64> {
    prices.get(currency).map(|amount| (*amount).into())
}

fn compute_ttl(created: &OffsetDateTime) -> i64 {
    (*created + time::Duration::days(7)).unix_timestamp()
}

impl From<Notification> for NotificationRecord {
    fn from(notification: Notification) -> Self {
        match notification.notification_payload {
            NotificationPayload::Watchlist {
                product_id,
                shop_id,
                shops_product_id,
                shop_slug_id,
                product_slug_id,
                shop_name,
                title,
                watchlist_payload,
            } => {
                let notification_reason = derive_notification_reason(&watchlist_payload);
                let lsi1_sk = mk_lsi1_sk(&notification.notification_id, &notification_reason);

                let (
                    new_price_native,
                    new_price_eur,
                    new_price_usd,
                    new_price_gbp,
                    new_price_aud,
                    new_price_cad,
                    new_price_nzd,
                    old_price_native,
                    old_price_eur,
                    old_price_usd,
                    old_price_gbp,
                    old_price_aud,
                    old_price_cad,
                    old_price_nzd,
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
                            extract_currency_amount(new_price, &Currency::Eur),
                            extract_currency_amount(new_price, &Currency::Usd),
                            extract_currency_amount(new_price, &Currency::Gbp),
                            extract_currency_amount(new_price, &Currency::Aud),
                            extract_currency_amount(new_price, &Currency::Cad),
                            extract_currency_amount(new_price, &Currency::Nzd),
                            old_native,
                            extract_currency_amount(old_price, &Currency::Eur),
                            extract_currency_amount(old_price, &Currency::Usd),
                            extract_currency_amount(old_price, &Currency::Gbp),
                            extract_currency_amount(old_price, &Currency::Aud),
                            extract_currency_amount(old_price, &Currency::Cad),
                            extract_currency_amount(old_price, &Currency::Nzd),
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
                        Some(ProductStateRecord::from(*new_state)),
                        Some(ProductStateRecord::from(*old_state)),
                    ),
                };

                NotificationRecord {
                    pk: mk_pk(&notification.user_id),
                    sk: mk_sk(&notification.notification_id),
                    lsi1_sk,
                    user_id: notification.user_id,
                    notification_id: notification.notification_id,
                    notification_reason,
                    seen: notification.seen,
                    image: None,
                    product_id: Some(product_id),
                    product_slug_id: Some(product_slug_id),
                    shop_slug_id: Some(shop_slug_id),
                    shop_id: Some(shop_id),
                    shops_product_id: Some(shops_product_id),
                    shop_name: Some(String::from(shop_name)),
                    title_de: title.get(&Language::De).map(|t| String::from(t.clone())),
                    title_en: title.get(&Language::En).map(|t| String::from(t.clone())),
                    title_fr: title.get(&Language::Fr).map(|t| String::from(t.clone())),
                    title_es: title.get(&Language::Es).map(|t| String::from(t.clone())),
                    title_it: title.get(&Language::It).map(|t| String::from(t.clone())),
                    new_price_native,
                    new_price_eur,
                    new_price_usd,
                    new_price_gbp,
                    new_price_aud,
                    new_price_cad,
                    new_price_nzd,
                    old_price_native,
                    old_price_eur,
                    old_price_usd,
                    old_price_gbp,
                    old_price_aud,
                    old_price_cad,
                    old_price_nzd,
                    new_state,
                    old_state,
                    created: notification.created,
                    updated: notification.updated,
                    ttl: compute_ttl(&notification.created),
                }
            }
        }
    }
}

fn build_price_map(
    native: Option<PriceRecord>,
    eur: Option<u64>,
    usd: Option<u64>,
    gbp: Option<u64>,
    aud: Option<u64>,
    cad: Option<u64>,
    nzd: Option<u64>,
) -> HashMap<Currency, MonetaryAmount> {
    let mut map = HashMap::new();
    if let Some(native) = native {
        let price: Price = native.into();
        map.insert(price.currency, price.monetary_amount);
    }
    if let Some(v) = eur {
        map.insert(Currency::Eur, MonetaryAmount::from(v));
    }
    if let Some(v) = usd {
        map.insert(Currency::Usd, MonetaryAmount::from(v));
    }
    if let Some(v) = gbp {
        map.insert(Currency::Gbp, MonetaryAmount::from(v));
    }
    if let Some(v) = aud {
        map.insert(Currency::Aud, MonetaryAmount::from(v));
    }
    if let Some(v) = cad {
        map.insert(Currency::Cad, MonetaryAmount::from(v));
    }
    if let Some(v) = nzd {
        map.insert(Currency::Nzd, MonetaryAmount::from(v));
    }
    map
}

impl From<NotificationRecord> for Notification {
    fn from(record: NotificationRecord) -> Self {
        let is_state_change = matches!(
            record.notification_reason,
            NotificationReasonRecord::WatchlistStateListed
                | NotificationReasonRecord::WatchlistStateAvailable
                | NotificationReasonRecord::WatchlistStateReserved
                | NotificationReasonRecord::WatchlistStateSold
                | NotificationReasonRecord::WatchlistStateRemoved
                | NotificationReasonRecord::WatchlistStateUnknown
        );

        let watchlist_payload = if is_state_change {
            NotificationWatchlistPayload::StateChange {
                old_state: record
                    .old_state
                    .map(ProductState::from)
                    .unwrap_or(ProductState::Unknown),
                new_state: record
                    .new_state
                    .map(ProductState::from)
                    .unwrap_or(ProductState::Unknown),
            }
        } else {
            NotificationWatchlistPayload::PriceChange {
                old_price: build_price_map(
                    record.old_price_native,
                    record.old_price_eur,
                    record.old_price_usd,
                    record.old_price_gbp,
                    record.old_price_aud,
                    record.old_price_cad,
                    record.old_price_nzd,
                ),
                new_price: build_price_map(
                    record.new_price_native,
                    record.new_price_eur,
                    record.new_price_usd,
                    record.new_price_gbp,
                    record.new_price_aud,
                    record.new_price_cad,
                    record.new_price_nzd,
                ),
            }
        };

        let mut title = HashMap::new();
        if let Some(t) = record.title_de {
            title.insert(Language::De, Title::from(t));
        }
        if let Some(t) = record.title_en {
            title.insert(Language::En, Title::from(t));
        }
        if let Some(t) = record.title_fr {
            title.insert(Language::Fr, Title::from(t));
        }
        if let Some(t) = record.title_es {
            title.insert(Language::Es, Title::from(t));
        }
        if let Some(t) = record.title_it {
            title.insert(Language::It, Title::from(t));
        }

        Notification {
            user_id: record.user_id,
            notification_id: record.notification_id,
            notification_payload: NotificationPayload::Watchlist {
                product_id: record.product_id.unwrap_or_default(),
                shop_id: record.shop_id.unwrap_or_default(),
                shops_product_id: record.shops_product_id.unwrap_or_default(),
                shop_slug_id: record.shop_slug_id.unwrap_or_else(|| SlugId::raw("")),
                product_slug_id: record.product_slug_id.unwrap_or_else(|| SlugId::raw("")),
                shop_name: record
                    .shop_name
                    .map(ShopName::from)
                    .unwrap_or_else(|| ShopName::from("")),
                title,
                watchlist_payload,
            },
            seen: record.seen,
            created: record.created,
            updated: record.updated,
        }
    }
}

#[cfg(feature = "test-data")]
mod faker {
    use super::*;
    use fake::{Dummy, Fake, Faker, Rng};

    impl Dummy<Faker> for NotificationRecord {
        fn dummy_with_rng<R: Rng + ?Sized>(config: &Faker, rng: &mut R) -> Self {
            let user_id: UserId = config.fake_with_rng(rng);
            let notification_id = NotificationId::new();
            let notification_reason: NotificationReasonRecord = config.fake_with_rng(rng);
            let created = OffsetDateTime::now_utc();

            NotificationRecord {
                pk: mk_pk(&user_id),
                sk: mk_sk(&notification_id),
                lsi1_sk: mk_lsi1_sk(&notification_id, &notification_reason),
                user_id,
                notification_id,
                notification_reason,
                seen: config.fake_with_rng(rng),
                image: None,
                product_id: Some(config.fake_with_rng(rng)),
                product_slug_id: Some(config.fake_with_rng(rng)),
                shop_slug_id: Some(config.fake_with_rng(rng)),
                shop_id: Some(config.fake_with_rng(rng)),
                shops_product_id: Some(config.fake_with_rng(rng)),
                shop_name: Some(config.fake_with_rng(rng)),
                title_de: Some(config.fake_with_rng(rng)),
                title_en: Some(config.fake_with_rng(rng)),
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
                old_price_native: None,
                old_price_eur: Some(config.fake_with_rng(rng)),
                old_price_usd: None,
                old_price_gbp: None,
                old_price_aud: None,
                old_price_cad: None,
                old_price_nzd: None,
                new_state: Some(config.fake_with_rng(rng)),
                old_state: Some(config.fake_with_rng(rng)),
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
    }
}

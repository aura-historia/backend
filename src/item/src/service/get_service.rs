use crate::core::description::Description;
use crate::core::item::{Item, LocalizedItemView};
use crate::core::item_event::{
    ItemEvent, ItemEventPayload, LocalizedItemCreatedEventPayloadView,
    LocalizedItemEventPayloadView, LocalizedItemPriceChangeEventPayloadView,
    LocalizedItemPriceDiscoveryEventPayloadView, LocalizedItemPriceRemovedEventPayloadView,
    LocalizedItemStateChangeEventPayloadView,
};
use crate::core::title::Title;
use crate::dynamodb::item_event_record::ItemEventRecord;
use crate::dynamodb::item_record::ItemRecord;
use crate::dynamodb::repository::ItemDynamoDbRepository;
use async_trait::async_trait;
use aws_sdk_dynamodb::config::http::HttpResponse;
use aws_sdk_dynamodb::error::SdkError;
use common::batch::Batch;
use common::currency::domain::Currency;
use common::event::Event;
use common::item_id::{ItemId, ItemKey};
use common::language::domain::Language;
use common::localized::Localized;
use common::price::domain::{MonetaryAmountOverflowError, Price};
use common::shop_id::ShopId;
use common::shops_item_id::ShopsItemId;
use std::collections::HashMap;
use tracing::error;

#[derive(thiserror::Error, Debug)]
pub enum GetItemError {
    #[error("Item with ShopId '{0}' and ShopsItemId '{1}' not found.")]
    ItemNotFound(ShopId, ShopsItemId),

    #[error("{0}")]
    MonetaryAmountOverflowError(#[from] MonetaryAmountOverflowError),

    #[error("Encountered DynamoDB SdkError for GetItem: {0}")]
    SdkGetItemError(
        #[from] SdkError<aws_sdk_dynamodb::operation::get_item::GetItemError, HttpResponse>,
    ),

    #[error("Encountered DynamoDB SdkError for BatchGetItem: {0}")]
    SdkBatchGetItemError(
        #[from]
        SdkError<aws_sdk_dynamodb::operation::batch_get_item::BatchGetItemError, HttpResponse>,
    ),

    #[error("Encountered DynamoDB SdkError for QueryItem: {0}")]
    SdkQueryError(#[from] SdkError<aws_sdk_dynamodb::operation::query::QueryError, HttpResponse>),

    #[error("Unable to resolve unprocessed items after '{0}' retries. Failing entire operation.")]
    UnprocessedAfterMaxRetries(u32),
}

#[cfg(feature = "data")]
pub mod api {
    use crate::service::get_service::GetItemError;
    use common::api::error::ApiError;
    use common::api::error_code::{
        ITEM_NOT_FOUND, MONETARY_AMOUNT_OVERFLOW, UNPROCESSED_AFTER_MAX_RETRIES,
    };
    use tracing::{error, warn};

    impl From<GetItemError> for ApiError {
        fn from(err: GetItemError) -> Self {
            match err {
                GetItemError::ItemNotFound(shop_id, ref shops_item_id) => {
                    warn!(error = %err, shopId = %shop_id, shopsItemId = %shops_item_id);
                    ApiError::not_found(ITEM_NOT_FOUND)
                }
                GetItemError::MonetaryAmountOverflowError(err) => {
                    error!(error = %err, "Encountered MonetaryAmountOverflowError while getting item.");
                    ApiError::internal_server_error(MONETARY_AMOUNT_OVERFLOW)
                }
                GetItemError::SdkGetItemError(err) => {
                    error!(error = ?err, "Encountered SdkGetItemError while getting item.");
                    err.into()
                }
                GetItemError::SdkBatchGetItemError(err) => {
                    error!(error = ?err, "Encountered SdkBatchGetItemError while getting item.");
                    err.into()
                }
                GetItemError::SdkQueryError(err) => {
                    error!(error = ?err, "Encountered SdkQueryError while querying item and its history.");
                    err.into()
                }
                GetItemError::UnprocessedAfterMaxRetries(_) => {
                    error!(error = %err, "Had unprocessed items for BatchGetItem after retries..");
                    ApiError::service_unavailable(UNPROCESSED_AFTER_MAX_RETRIES)
                }
            }
        }
    }
}

#[async_trait]
#[mockall::automock]
pub trait GetItemService {
    async fn find_item(
        &self,
        shop_id: &ShopId,
        shops_item_id: &ShopsItemId,
    ) -> Result<Item, GetItemError>;

    async fn view_item(
        &self,
        shop_id: &ShopId,
        shops_item_id: &ShopsItemId,
        languages: &[Language],
        currency: &Currency,
        history: bool,
    ) -> Result<LocalizedItemView, GetItemError>;

    async fn view_items(
        &self,
        items: Vec<ItemKey>,
        languages: &[Language],
        currency: &Currency,
    ) -> Result<Vec<LocalizedItemView>, GetItemError>;
}

pub struct GetItemServiceImpl<'a> {
    repository: &'a (dyn ItemDynamoDbRepository + Sync),
}

impl<'a> GetItemServiceImpl<'a> {
    pub fn new(repository: &'a (dyn ItemDynamoDbRepository + Sync)) -> Self {
        Self { repository }
    }
}

#[async_trait]
impl<'a> GetItemService for GetItemServiceImpl<'a> {
    async fn find_item(
        &self,
        shop_id: &ShopId,
        shops_item_id: &ShopsItemId,
    ) -> Result<Item, GetItemError> {
        let item_record = self
            .repository
            .get_item_record(shop_id, shops_item_id)
            .await?
            .ok_or(GetItemError::ItemNotFound(*shop_id, shops_item_id.clone()))?;

        Ok(item_record.into())
    }

    #[tracing::instrument(skip(self), fields(shopId = %shop_id, shopsItemId = %shops_item_id))]
    async fn view_item(
        &self,
        shop_id: &ShopId,
        shops_item_id: &ShopsItemId,
        preferred_languages: &[Language],
        currency: &Currency,
        history: bool,
    ) -> Result<LocalizedItemView, GetItemError> {
        let (item_record, event_records) = if history {
            self.repository
                .query_item_record_and_event_records(shop_id, shops_item_id)
                .await?
                .ok_or(GetItemError::ItemNotFound(*shop_id, shops_item_id.clone()))?
        } else {
            let item_record = self
                .repository
                .get_item_record(shop_id, shops_item_id)
                .await?
                .ok_or(GetItemError::ItemNotFound(*shop_id, shops_item_id.clone()))?;
            (item_record, vec![])
        };

        let mut item_view = localize_item_record(item_record, currency, preferred_languages);

        let event_views = event_records
            .into_iter()
            .map(ItemEvent::try_from)
            .filter_map(|event_res| match event_res {
                Ok(event) => Some(event),
                Err(err) => {
                    error!(
                        error = %err,
                        fromType = %std::any::type_name::<ItemEventRecord>(),
                        toType = %std::any::type_name::<ItemEvent>(),
                        "Failed mapping types."
                    );
                    None
                }
            })
            .map(|event| localize_item_event(event, preferred_languages, currency))
            .collect();
        item_view.history = if history { Some(event_views) } else { None };

        Ok(item_view)
    }

    async fn view_items(
        &self,
        items: Vec<ItemKey>,
        languages: &[Language],
        currency: &Currency,
    ) -> Result<Vec<LocalizedItemView>, GetItemError> {
        const MAX_RETRIES: u32 = 3;
        const BASE_DELAY_MS: u64 = 100;

        let mut views = Vec::with_capacity(items.len());
        let mut unprocessed = items;
        let mut retry_count = 0;
        loop {
            let (mut local_views, local_unprocessed) = self
                .view_items_with_unprocessed(unprocessed, languages, currency)
                .await?;
            views.append(&mut local_views);

            if local_unprocessed.is_empty() {
                break;
            } else if retry_count >= MAX_RETRIES {
                return Err(GetItemError::UnprocessedAfterMaxRetries(MAX_RETRIES));
            }

            retry_count += 1;
            let delay_ms = BASE_DELAY_MS * 2_u64.pow(retry_count - 1);
            tokio::time::sleep(tokio::time::Duration::from_millis(delay_ms)).await;

            unprocessed = local_unprocessed;
        }

        Ok(views)
    }
}

impl<'a> GetItemServiceImpl<'a> {
    async fn view_items_with_unprocessed(
        &self,
        items: Vec<ItemKey>,
        languages: &[Language],
        currency: &Currency,
    ) -> Result<(Vec<LocalizedItemView>, Vec<ItemKey>), GetItemError> {
        let mut views = Vec::with_capacity(items.len());
        let mut unprocessed = Vec::new();
        for batch in Batch::chunked_from(items.into_iter()) {
            let result = self.repository.get_item_records(&batch).await?;
            if let Some(up) = result.unprocessed {
                unprocessed.extend(up);
            }
            let local_views = result
                .items
                .into_iter()
                .map(|record| localize_item_record(record, currency, languages));
            views.extend(local_views);
        }

        Ok((views, unprocessed))
    }
}

fn localize_item_record(
    item_record: ItemRecord,
    currency: &Currency,
    preferred_languages: &[Language],
) -> LocalizedItemView {
    let mut available_titles: HashMap<Language, Title> = HashMap::with_capacity(3);
    available_titles.insert(
        item_record.title_native.language.into(),
        item_record.title_native.text.into(),
    );
    if let Some(title_de) = item_record.title_de {
        available_titles.insert(Language::De, title_de.into());
    }
    if let Some(title_en) = item_record.title_en {
        available_titles.insert(Language::En, title_en.into());
    }

    let mut available_descriptions: HashMap<Language, Description> = HashMap::with_capacity(3);
    if let Some(description_native) = item_record.description_native {
        available_descriptions.insert(
            description_native.language.into(),
            description_native.text.into(),
        );
    }
    if let Some(description_de) = item_record.description_de {
        available_descriptions.insert(Language::De, description_de.into());
    }
    if let Some(description_en) = item_record.description_en {
        available_descriptions.insert(Language::En, description_en.into());
    }

    let title = Language::resolve(preferred_languages, available_titles).unwrap_or_else(|| {
        error!("Failed resolving title. This SHOULD be impossible because the native title always exists.");
        Localized::new(Language::En, "Unknown title".into())
    });
    let description = Language::resolve(preferred_languages, available_descriptions);

    let price = match currency {
        Currency::Eur => item_record
            .price_eur
            .map(|amount| Price::new(amount.into(), Currency::Eur)),
        Currency::Gbp => item_record
            .price_gbp
            .map(|amount| Price::new(amount.into(), Currency::Gbp)),
        Currency::Usd => item_record
            .price_usd
            .map(|amount| Price::new(amount.into(), Currency::Usd)),
        Currency::Aud => item_record
            .price_aud
            .map(|amount| Price::new(amount.into(), Currency::Aud)),
        Currency::Cad => item_record
            .price_cad
            .map(|amount| Price::new(amount.into(), Currency::Cad)),
        Currency::Nzd => item_record
            .price_nzd
            .map(|amount| Price::new(amount.into(), Currency::Nzd)),
    };

    LocalizedItemView {
        item_id: item_record.item_id,
        event_id: item_record.event_id,
        shop_id: item_record.shop_id,
        shops_item_id: item_record.shops_item_id,
        shop_name: item_record.shop_name.into(),
        title,
        description,
        price,
        state: item_record.state.into(),
        url: item_record.url,
        images: item_record.images,
        created: item_record.created,
        updated: item_record.updated,
        history: None,
    }
}

fn localize_item_event(
    event: ItemEvent,
    preferred_languages: &[Language],
    currency: &Currency,
) -> Event<ItemId, LocalizedItemEventPayloadView> {
    let payload = match event.payload {
        ItemEventPayload::Created(payload) => {
            let mut titles = payload.other_title;
            titles.insert(
                payload.native_title.localization,
                payload.native_title.payload,
            );
            let mut descriptions = payload.other_description;
            if let Some(native_description) = payload.native_description {
                descriptions.insert(native_description.localization, native_description.payload);
            }
            let mut prices = payload.other_price;
            if let Some(native_price) = payload.native_price {
                prices.insert(native_price.currency, native_price.monetary_amount);
            }
            LocalizedItemEventPayloadView::Created(LocalizedItemCreatedEventPayloadView {
                    shop_id: payload.shop_id,
                    shops_item_id: payload.shops_item_id,
                    shop_name: payload.shop_name,
                    title: Language::resolve(preferred_languages, titles)
                        .unwrap_or_else(|| {
                            error!("Failed resolving title. This SHOULD be impossible because the native title always exists.");
                            Localized::new(Language::En, "Unknown title".into())
                        }),
                    description: Language::resolve(preferred_languages, descriptions),
                    price: prices
                        .remove(currency)
                        .map(|amount| Price::new(amount, *currency)),
                    state: payload.state,
                    url: payload.url,
                    images: payload.images,
                })
        }
        ItemEventPayload::StateListed(payload) => {
            LocalizedItemEventPayloadView::StateListed(LocalizedItemStateChangeEventPayloadView {
                shop_id: payload.shop_id,
                shops_item_id: payload.shops_item_id,
                old_state: payload.old_state,
            })
        }
        ItemEventPayload::StateAvailable(payload) => LocalizedItemEventPayloadView::StateAvailable(
            LocalizedItemStateChangeEventPayloadView {
                shop_id: payload.shop_id,
                shops_item_id: payload.shops_item_id,
                old_state: payload.old_state,
            },
        ),
        ItemEventPayload::StateReserved(payload) => {
            LocalizedItemEventPayloadView::StateReserved(LocalizedItemStateChangeEventPayloadView {
                shop_id: payload.shop_id,
                shops_item_id: payload.shops_item_id,
                old_state: payload.old_state,
            })
        }
        ItemEventPayload::StateSold(payload) => {
            LocalizedItemEventPayloadView::StateSold(LocalizedItemStateChangeEventPayloadView {
                shop_id: payload.shop_id,
                shops_item_id: payload.shops_item_id,
                old_state: payload.old_state,
            })
        }
        ItemEventPayload::StateRemoved(payload) => {
            LocalizedItemEventPayloadView::StateRemoved(LocalizedItemStateChangeEventPayloadView {
                shop_id: payload.shop_id,
                shops_item_id: payload.shops_item_id,
                old_state: payload.old_state,
            })
        }
        ItemEventPayload::StateUnknown(payload) => {
            LocalizedItemEventPayloadView::StateUnknown(LocalizedItemStateChangeEventPayloadView {
                shop_id: payload.shop_id,
                shops_item_id: payload.shops_item_id,
                old_state: payload.old_state,
            })
        }
        ItemEventPayload::PriceDiscovered(payload) => {
            let mut prices = payload.other_price;
            prices.insert(
                payload.native_price.currency,
                payload.native_price.monetary_amount,
            );
            LocalizedItemEventPayloadView::PriceDiscovered(
                    LocalizedItemPriceDiscoveryEventPayloadView {
                        shop_id: payload.shop_id,
                        shops_item_id: payload.shops_item_id,
                        price: Currency::resolve(&[*currency], prices).unwrap_or_else(|| {
                            error!("Failed resolving price. This SHOULD be impossible because the native price always exists.");
                            Price::new(0u64.into(), *currency)
                        }),
                    },
                )
        }
        ItemEventPayload::PriceDropped(payload) => {
            let mut new_prices = payload.new_other_price;
            new_prices.insert(
                payload.new_native_price.currency,
                payload.new_native_price.monetary_amount,
            );
            let mut old_prices = payload.old_other_price;
            old_prices.insert(
                payload.old_native_price.currency,
                payload.old_native_price.monetary_amount,
            );
            LocalizedItemEventPayloadView::PriceDropped(
                    LocalizedItemPriceChangeEventPayloadView {
                        shop_id: payload.shop_id,
                        shops_item_id: payload.shops_item_id,
                        new_price: Currency::resolve(&[*currency], new_prices).unwrap_or_else(|| {
                            error!("Failed resolving price. This SHOULD be impossible because the native price always exists.");
                            Price::new(0u64.into(), *currency)
                        }),
                        old_price: Currency::resolve(&[*currency], old_prices).unwrap_or_else(|| {
                            error!("Failed resolving price. This SHOULD be impossible because the native price always exists.");
                            Price::new(0u64.into(), *currency)
                        }),
                    },
                )
        }
        ItemEventPayload::PriceIncreased(payload) => {
            let mut new_prices = payload.new_other_price;
            new_prices.insert(
                payload.new_native_price.currency,
                payload.new_native_price.monetary_amount,
            );
            let mut old_prices = payload.old_other_price;
            old_prices.insert(
                payload.old_native_price.currency,
                payload.old_native_price.monetary_amount,
            );
            LocalizedItemEventPayloadView::PriceIncreased(
                    LocalizedItemPriceChangeEventPayloadView {
                        shop_id: payload.shop_id,
                        shops_item_id: payload.shops_item_id,
                        new_price: Currency::resolve(&[*currency], new_prices).unwrap_or_else(|| {
                            error!("Failed resolving price. This SHOULD be impossible because the native price always exists.");
                            Price::new(0u64.into(), *currency)
                        }),
                        old_price: Currency::resolve(&[*currency], old_prices).unwrap_or_else(|| {
                            error!("Failed resolving price. This SHOULD be impossible because the native price always exists.");
                            Price::new(0u64.into(), *currency)
                        }),
                    },
                )
        }
        ItemEventPayload::PriceRemoved(payload) => {
            let mut old_prices = payload.old_other_price;
            old_prices.insert(
                payload.old_native_price.currency,
                payload.old_native_price.monetary_amount,
            );
            LocalizedItemEventPayloadView::PriceRemoved(LocalizedItemPriceRemovedEventPayloadView {
                shop_id: payload.shop_id,
                shops_item_id: payload.shops_item_id,
                old_price: Currency::resolve(&[*currency], old_prices).unwrap_or_else(|| {
                    error!("Failed resolving price. This SHOULD be impossible because the native price always exists.");
                    Price::new(0u64.into(), *currency)
                }),
            })
        }
    };
    Event {
        aggregate_id: event.aggregate_id,
        event_id: event.event_id,
        timestamp: event.timestamp,
        payload,
    }
}

#[cfg(test)]
mod tests {
    mod find_item {
        use crate::dynamodb::repository::MockItemDynamoDbRepository;
        use crate::service::get_service::{GetItemError, GetItemService, GetItemServiceImpl};
        use aws_sdk_dynamodb::{
            config::http::HttpResponse,
            error::{ConnectorError, SdkError},
        };
        use common::{shop_id::ShopId, shops_item_id::ShopsItemId};
        use fake::{Fake, Faker};

        #[tokio::test]
        async fn should_return_item_when_exists() {
            let mut repository = MockItemDynamoDbRepository::default();
            repository
                .expect_get_item_record()
                .return_once(|_, _| Box::pin(async { Ok(Some(Faker.fake())) }));
            let service = GetItemServiceImpl {
                repository: &repository,
            };
            let actual = service.find_item(&ShopId::new(), &ShopsItemId::new()).await;
            assert!(actual.is_ok());
        }

        #[tokio::test]
        async fn should_return_item_not_found_err_when_item_does_not_exist() {
            let shop_id = ShopId::new();
            let shops_item_id = "non-existent".into();
            let mut repository = MockItemDynamoDbRepository::default();
            repository
                .expect_get_item_record()
                .return_once(|_, _| Box::pin(async { Ok(None) }));
            let service = GetItemServiceImpl {
                repository: &repository,
            };
            let actual = service.find_item(&shop_id, &shops_item_id).await;

            assert!(actual.is_err());
            match actual.unwrap_err() {
                GetItemError::ItemNotFound(err_shop_id, err_shops_item_id) => {
                    assert_eq!(err_shop_id, shop_id);
                    assert_eq!(err_shops_item_id, shops_item_id);
                }
                _ => panic!("expected GetItemError::ItemNotFound"),
            }
        }

        #[tokio::test]
        #[rstest::rstest]
        #[case::construction_failure(SdkError::construction_failure("Something went wrong"))]
        #[case::timeout(SdkError::timeout_error("Something went wrong"))]
        #[case::dispatch_failure(SdkError::dispatch_failure(ConnectorError::user("Something went wrong".into())))]
        #[case::response_error(SdkError::response_error(
            "Something went wrong",
            HttpResponse::new(500u16.try_into().unwrap(), "{}".into())
        ))]
        #[case::service_error(SdkError::service_error(
            aws_sdk_dynamodb::operation::get_item::GetItemError::unhandled("Something went wrong"),
            HttpResponse::new(500u16.try_into().unwrap(), "{}".into())
        ))]
        async fn should_propagate_sdk_error(
            #[case] expected: SdkError<
                aws_sdk_dynamodb::operation::get_item::GetItemError,
                aws_sdk_dynamodb::config::http::HttpResponse,
            >,
        ) {
            let shop_id = ShopId::new();
            let shops_item_id = "non-existent".into();
            let mut repository = MockItemDynamoDbRepository::default();
            repository
                .expect_get_item_record()
                .return_once(|_, _| Box::pin(async { Err(expected) }));
            let service = GetItemServiceImpl {
                repository: &repository,
            };
            let actual = service.find_item(&shop_id, &shops_item_id).await;

            assert!(actual.is_err());
            match actual.unwrap_err() {
                GetItemError::SdkGetItemError(_) => {}
                _ => panic!("expected GetItemError::ItemNotFound"),
            }
        }
    }

    mod view_item {
        use crate::dynamodb::{
            item_event_record::ItemEventRecord, item_record::ItemRecord,
            repository::MockItemDynamoDbRepository,
        };
        use crate::service::get_service::{GetItemError, GetItemService, GetItemServiceImpl};
        use aws_sdk_dynamodb::{
            config::http::HttpResponse,
            error::{ConnectorError, SdkError},
        };
        use common::{
            currency::domain::Currency,
            language::{
                domain::Language,
                domain::Language::*,
                record::{LanguageRecord, TextRecord},
            },
            shop_id::ShopId,
            shops_item_id::ShopsItemId,
        };
        use fake::{Fake, Faker};
        use itertools::Itertools;

        #[tokio::test]
        async fn should_return_item_when_exists_without_history() {
            let mut repository = MockItemDynamoDbRepository::default();
            repository
                .expect_get_item_record()
                .return_once(|_, _| Box::pin(async { Ok(Some(Faker.fake())) }));
            let service = GetItemServiceImpl {
                repository: &repository,
            };
            let actual = service
                .view_item(
                    &ShopId::new(),
                    &ShopsItemId::new(),
                    &[],
                    &Currency::Eur,
                    false,
                )
                .await;
            assert!(actual.is_ok());
        }

        #[tokio::test]
        async fn should_return_item_when_exists_with_history() {
            let mut repository = MockItemDynamoDbRepository::default();
            repository
                .expect_query_item_record_and_event_records()
                .return_once(|_, _| Box::pin(async { Ok(Some(Faker.fake())) }));
            let service = GetItemServiceImpl {
                repository: &repository,
            };
            let actual = service
                .view_item(
                    &ShopId::new(),
                    &ShopsItemId::new(),
                    &[],
                    &Currency::Eur,
                    true,
                )
                .await;
            assert!(actual.is_ok());
        }

        #[tokio::test]
        async fn should_keep_history_in_exact_order_as_dynamodb_read() {
            let mut repository = MockItemDynamoDbRepository::default();
            let mut events = fake::vec![ItemEventRecord; 100];
            events.sort_by(|l, r| l.item_id.cmp(&r.item_id));
            let expected = events
                .clone()
                .into_iter()
                .map(|record| record.item_id)
                .collect_vec();
            repository
                .expect_query_item_record_and_event_records()
                .return_once(|_, _| Box::pin(async { Ok(Some((Faker.fake(), events))) }));
            let service = GetItemServiceImpl {
                repository: &repository,
            };
            let actual = service
                .view_item(
                    &ShopId::new(),
                    &ShopsItemId::new(),
                    &[],
                    &Currency::Eur,
                    true,
                )
                .await
                .unwrap()
                .history
                .unwrap()
                .into_iter()
                .map(|event| event.aggregate_id)
                .collect_vec();

            assert_eq!(expected, actual);
        }

        #[tokio::test]
        #[rstest::rstest]
        #[case::eur(Currency::Eur, 2)]
        #[case::gbp(Currency::Gbp, 4)]
        #[case::usd(Currency::Usd, 10)]
        #[case::aud(Currency::Aud, 1000)]
        #[case::cad(Currency::Cad, 4000)]
        #[case::nzd(Currency::Nzd, 42)]
        async fn should_respect_currency(#[case] currency: Currency, #[case] expected_amount: u64) {
            let mut repository = MockItemDynamoDbRepository::default();
            let mut expected_record: ItemRecord = Faker.fake();
            expected_record.price_eur = Some(2);
            expected_record.price_gbp = Some(4);
            expected_record.price_usd = Some(10);
            expected_record.price_aud = Some(1000);
            expected_record.price_cad = Some(4000);
            expected_record.price_nzd = Some(42);
            repository
                .expect_get_item_record()
                .return_once(|_, _| Box::pin(async { Ok(Some(expected_record)) }));
            let service = GetItemServiceImpl {
                repository: &repository,
            };
            let actual_price = service
                .view_item(&ShopId::new(), &ShopsItemId::new(), &[], &currency, false)
                .await
                .unwrap()
                .price
                .unwrap();
            assert_eq!(currency, actual_price.currency);
            assert_eq!(expected_amount, u64::from(actual_price.monetary_amount));
        }

        #[tokio::test]
        #[rstest::rstest]
        #[case(&[], De, "German")]
        #[case(&[De], De, "German")]
        #[case(&[De, En], De, "German")]
        #[case(&[De, Fr], De, "German")]
        #[case(&[Fr, De, En, Es], De, "German")]
        #[case(&[En], En, "English")]
        #[case(&[En, De, Fr, Es], En, "English")]
        #[case(&[En, De, Es], En, "English")]
        #[case(&[Es, De, En], Es, "Spanish")]
        #[case(&[Es, En, De], Es, "Spanish")]
        async fn should_respect_language_for_title(
            #[case] languages: &[Language],
            #[case] expected_language: Language,
            #[case] expected_title: &str,
        ) {
            let mut repository = MockItemDynamoDbRepository::default();
            let mut expected_record: ItemRecord = Faker.fake();
            expected_record.title_native = TextRecord::new("Spanish", LanguageRecord::Es);
            expected_record.title_de = Some("German".to_string());
            expected_record.title_en = Some("English".to_string());
            repository
                .expect_get_item_record()
                .return_once(|_, _| Box::pin(async { Ok(Some(expected_record)) }));
            let service = GetItemServiceImpl {
                repository: &repository,
            };
            let actual_title = service
                .view_item(
                    &ShopId::new(),
                    &ShopsItemId::new(),
                    languages,
                    &Currency::Gbp,
                    false,
                )
                .await
                .unwrap()
                .title;
            assert_eq!(expected_language, actual_title.localization);
            assert_eq!(expected_title, actual_title.payload.as_ref());
        }

        #[tokio::test]
        #[rstest::rstest]
        #[case(&[], Es, "Spanish")]
        #[case(&[De], Es, "Spanish")]
        #[case(&[De, En], Es, "Spanish")]
        #[case(&[De, Fr], Es, "Spanish")]
        #[case(&[Fr, De, En, Es], Es, "Spanish")]
        #[case(&[En], Es, "Spanish")]
        #[case(&[En, De, Fr, Es], Es, "Spanish")]
        #[case(&[En, De, Es], Es, "Spanish")]
        #[case(&[Es, De, En], Es, "Spanish")]
        #[case(&[Es, En, De], Es, "Spanish")]
        async fn should_fallback_to_native_when_only_native_exists_for_title(
            #[case] languages: &[Language],
            #[case] expected_language: Language,
            #[case] expected_title: &str,
        ) {
            let mut repository = MockItemDynamoDbRepository::default();
            let mut expected_record: ItemRecord = Faker.fake();
            expected_record.title_native = TextRecord::new("Spanish", LanguageRecord::Es);
            expected_record.title_de = None;
            expected_record.title_en = None;
            repository
                .expect_get_item_record()
                .return_once(|_, _| Box::pin(async { Ok(Some(expected_record)) }));
            let service = GetItemServiceImpl {
                repository: &repository,
            };
            let actual_title = service
                .view_item(
                    &ShopId::new(),
                    &ShopsItemId::new(),
                    languages,
                    &Currency::Gbp,
                    false,
                )
                .await
                .unwrap()
                .title;
            assert_eq!(expected_language, actual_title.localization);
            assert_eq!(expected_title, actual_title.payload.as_ref());
        }

        #[tokio::test]
        #[rstest::rstest]
        #[case(&[], De, "German")]
        #[case(&[De], De, "German")]
        #[case(&[De, En], De, "German")]
        #[case(&[De, Fr], De, "German")]
        #[case(&[Fr, De, En, Es], De, "German")]
        #[case(&[En], En, "English")]
        #[case(&[En, De, Fr, Es], En, "English")]
        #[case(&[En, De, Es], En, "English")]
        #[case(&[Es, De, En], Es, "Spanish")]
        #[case(&[Es, En, De], Es, "Spanish")]
        async fn should_respect_language_for_description(
            #[case] languages: &[Language],
            #[case] expected_language: Language,
            #[case] expected_description: &str,
        ) {
            let mut repository = MockItemDynamoDbRepository::default();
            let mut expected_record: ItemRecord = Faker.fake();
            expected_record.description_native =
                Some(TextRecord::new("Spanish", LanguageRecord::Es));
            expected_record.description_de = Some("German".to_string());
            expected_record.description_en = Some("English".to_string());
            repository
                .expect_get_item_record()
                .return_once(|_, _| Box::pin(async { Ok(Some(expected_record)) }));
            let service = GetItemServiceImpl {
                repository: &repository,
            };
            let actual_description = service
                .view_item(
                    &ShopId::new(),
                    &ShopsItemId::new(),
                    languages,
                    &Currency::Gbp,
                    false,
                )
                .await
                .unwrap()
                .description
                .unwrap();
            assert_eq!(expected_language, actual_description.localization);
            assert_eq!(expected_description, actual_description.payload.as_ref());
        }

        #[tokio::test]
        #[rstest::rstest]
        #[case(&[], Es, "Spanish")]
        #[case(&[De], Es, "Spanish")]
        #[case(&[De, En], Es, "Spanish")]
        #[case(&[De, Fr], Es, "Spanish")]
        #[case(&[Fr, De, En, Es], Es, "Spanish")]
        #[case(&[En], Es, "Spanish")]
        #[case(&[En, De, Fr, Es], Es, "Spanish")]
        #[case(&[En, De, Es], Es, "Spanish")]
        #[case(&[Es, De, En], Es, "Spanish")]
        #[case(&[Es, En, De], Es, "Spanish")]
        async fn should_fallback_to_native_when_only_native_exists_for_description(
            #[case] languages: &[Language],
            #[case] expected_language: Language,
            #[case] expected_description: &str,
        ) {
            let mut repository = MockItemDynamoDbRepository::default();
            let mut expected_record: ItemRecord = Faker.fake();
            expected_record.description_native =
                Some(TextRecord::new("Spanish", LanguageRecord::Es));
            expected_record.description_de = None;
            expected_record.description_en = None;
            repository
                .expect_get_item_record()
                .return_once(|_, _| Box::pin(async { Ok(Some(expected_record)) }));
            let service = GetItemServiceImpl {
                repository: &repository,
            };
            let actual_description = service
                .view_item(
                    &ShopId::new(),
                    &ShopsItemId::new(),
                    languages,
                    &Currency::Gbp,
                    false,
                )
                .await
                .unwrap()
                .description
                .unwrap();
            assert_eq!(expected_language, actual_description.localization);
            assert_eq!(expected_description, actual_description.payload.as_ref());
        }

        #[tokio::test]
        #[rstest::rstest]
        #[case(&[])]
        #[case(&[De])]
        #[case(&[De, En])]
        #[case(&[De, Fr])]
        #[case(&[Fr, De, En, Es])]
        #[case(&[En])]
        #[case(&[En, De, Fr, Es])]
        #[case(&[En, De, Es])]
        #[case(&[Es, De, En])]
        #[case(&[Es, En, De])]
        async fn should_return_item_without_description_when_none_exists(
            #[case] languages: &[Language],
        ) {
            let mut repository = MockItemDynamoDbRepository::default();
            let mut expected_record: ItemRecord = Faker.fake();
            expected_record.description_native = None;
            expected_record.description_de = None;
            expected_record.description_en = None;
            repository
                .expect_get_item_record()
                .return_once(|_, _| Box::pin(async { Ok(Some(expected_record)) }));
            let service = GetItemServiceImpl {
                repository: &repository,
            };
            let actual_description = service
                .view_item(
                    &ShopId::new(),
                    &ShopsItemId::new(),
                    languages,
                    &Currency::Gbp,
                    false,
                )
                .await
                .unwrap()
                .description;
            assert!(actual_description.is_none());
        }

        #[tokio::test]
        async fn should_return_item_not_found_err_when_item_does_not_exist() {
            let shop_id = ShopId::new();
            let shops_item_id = "non-existent".into();
            let mut repository = MockItemDynamoDbRepository::default();
            repository
                .expect_get_item_record()
                .return_once(|_, _| Box::pin(async { Ok(None) }));
            let service = GetItemServiceImpl {
                repository: &repository,
            };
            let actual = service
                .view_item(&shop_id, &shops_item_id, &[], &Currency::Eur, false)
                .await;

            assert!(actual.is_err());
            match actual.unwrap_err() {
                GetItemError::ItemNotFound(err_shop_id, err_shops_item_id) => {
                    assert_eq!(err_shop_id, shop_id);
                    assert_eq!(err_shops_item_id, shops_item_id);
                }
                _ => panic!("expected GetItemError::ItemNotFound"),
            }
        }

        #[tokio::test]
        #[rstest::rstest]
        #[case::construction_failure(SdkError::construction_failure("Something went wrong"))]
        #[case::timeout(SdkError::timeout_error("Something went wrong"))]
        #[case::dispatch_failure(SdkError::dispatch_failure(ConnectorError::user("Something went wrong".into())))]
        #[case::response_error(SdkError::response_error(
            "Something went wrong",
            HttpResponse::new(500u16.try_into().unwrap(), "{}".into())
        ))]
        #[case::service_error(SdkError::service_error(
            aws_sdk_dynamodb::operation::get_item::GetItemError::unhandled("Something went wrong"),
            HttpResponse::new(500u16.try_into().unwrap(), "{}".into())
        ))]
        async fn should_propagate_sdk_error(
            #[case] expected: SdkError<
                aws_sdk_dynamodb::operation::get_item::GetItemError,
                aws_sdk_dynamodb::config::http::HttpResponse,
            >,
        ) {
            let shop_id = ShopId::new();
            let shops_item_id = "non-existent".into();
            let mut repository = MockItemDynamoDbRepository::default();
            repository
                .expect_get_item_record()
                .return_once(|_, _| Box::pin(async { Err(expected) }));
            let service = GetItemServiceImpl {
                repository: &repository,
            };
            let actual = service
                .view_item(&shop_id, &shops_item_id, &[], &Currency::Eur, false)
                .await;

            assert!(actual.is_err());
            match actual.unwrap_err() {
                GetItemError::SdkGetItemError(_) => {}
                _ => panic!("expected GetItemError::ItemNotFound"),
            }
        }
    }

    mod view_items {
        use crate::dynamodb::{item_record::ItemRecord, repository::MockItemDynamoDbRepository};
        use crate::service::get_service::{GetItemError, GetItemService, GetItemServiceImpl};
        use aws_sdk_dynamodb::{
            config::http::HttpResponse,
            error::{ConnectorError, SdkError},
        };
        use common::{batch::Batch, currency::domain::Currency};
        use common::{batch::dynamodb::BatchGetItemResult, item_id::ItemKey};

        #[tokio::test]
        async fn should_return_items_when_all_processed() {
            let mut repository = MockItemDynamoDbRepository::default();
            repository.expect_get_item_records().returning(|_| {
                Box::pin(async {
                    Ok(BatchGetItemResult {
                        items: fake::vec![ItemRecord; 42],
                        unprocessed: None,
                    })
                })
            });
            let service = GetItemServiceImpl {
                repository: &repository,
            };
            let actual = service
                .view_items(fake::vec![ItemKey; 42], &[], &Currency::Eur)
                .await
                .unwrap();
            assert_eq!(42, actual.len());
        }

        #[tokio::test]
        async fn should_completely_fail_when_some_processed() {
            let mut repository = MockItemDynamoDbRepository::default();
            repository.expect_get_item_records().returning(|_| {
                Box::pin(async {
                    Ok(BatchGetItemResult {
                        items: fake::vec![ItemRecord; 37],
                        unprocessed: Some(
                            Batch::try_from_iter(fake::vec![ItemKey; 5].into_iter()).unwrap(),
                        ),
                    })
                })
            });
            let service = GetItemServiceImpl {
                repository: &repository,
            };
            let actual = service
                .view_items(fake::vec![ItemKey; 42], &[], &Currency::Eur)
                .await
                .unwrap_err();
            match actual {
                GetItemError::UnprocessedAfterMaxRetries(_) => {}
                other => {
                    panic!("Expected 'GetItemError::UnprocessedAfterMaxRetries'. Got '{other:?}'.")
                }
            }
        }

        #[tokio::test]
        #[rstest::rstest]
        #[case::construction_failure(SdkError::construction_failure("Something went wrong"))]
        #[case::timeout(SdkError::timeout_error("Something went wrong"))]
        #[case::dispatch_failure(SdkError::dispatch_failure(ConnectorError::user("Something went wrong".into())))]
        #[case::response_error(SdkError::response_error(
            "Something went wrong",
            HttpResponse::new(500u16.try_into().unwrap(), "{}".into())
        ))]
        #[case::service_error(SdkError::service_error(
            aws_sdk_dynamodb::operation::batch_get_item::BatchGetItemError::unhandled("Something went wrong"),
            HttpResponse::new(500u16.try_into().unwrap(), "{}".into())
        ))]
        async fn should_fail_entire_operation_and_propagate_sdk_error_when_batch_operation_fails(
            #[case] expected: SdkError<
                aws_sdk_dynamodb::operation::batch_get_item::BatchGetItemError,
                aws_sdk_dynamodb::config::http::HttpResponse,
            >,
        ) {
            let mut repository = MockItemDynamoDbRepository::default();
            repository
                .expect_get_item_records()
                .return_once(|_| Box::pin(async { Err(expected) }));
            let service = GetItemServiceImpl {
                repository: &repository,
            };
            let actual = service
                .view_items(fake::vec![ItemKey; 222], &[], &Currency::Eur)
                .await;

            assert!(actual.is_err());
            match actual.unwrap_err() {
                GetItemError::SdkBatchGetItemError(_) => {}
                _ => panic!("expected GetItemError::SdkBatchGetItemError"),
            }
        }
    }
}

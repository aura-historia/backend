use crate::core::description::Description;
use crate::core::product::{LocalizedProductView, Product};
use crate::core::product_event::{
    LocalizedProductCreatedEventPayloadView, LocalizedProductEventPayloadView,
    LocalizedProductPriceChangeEventPayloadView, LocalizedProductPriceDiscoveryEventPayloadView,
    LocalizedProductPriceRemovedEventPayloadView, LocalizedProductStateChangeEventPayloadView,
    ProductEvent, ProductEventPayload,
};
use crate::core::title::Title;
use crate::dynamodb::product_event_record::ProductEventRecord;
use crate::dynamodb::product_record::ProductRecord;
use crate::dynamodb::repository::ProductDynamoDbRepository;
use async_trait::async_trait;
use aws_sdk_dynamodb::config::http::HttpResponse;
use aws_sdk_dynamodb::error::SdkError;
use common::batch::Batch;
use common::currency::domain::Currency;
use common::event::Event;
use common::language::domain::Language;
use common::localized::Localized;
use common::price::domain::{MonetaryAmountOverflowError, Price};
use common::product_id::{ProductId, ProductKey};
use common::shop_id::ShopId;
use common::shops_product_id::ShopsProductId;
use std::collections::HashMap;
use strum::EnumCount;
use tracing::error;

#[derive(thiserror::Error, Debug)]
pub enum GetProductError {
    #[error("Product with ShopId '{0}' and ShopsProductId '{1}' not found.")]
    ProductNotFound(ShopId, ShopsProductId),

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
    use crate::service::get_service::GetProductError;
    use common::api::error::ApiError;
    use common::api::error_code::{
        MONETARY_AMOUNT_OVERFLOW, PRODUCT_NOT_FOUND, UNPROCESSED_AFTER_MAX_RETRIES,
    };

    impl From<GetProductError> for ApiError {
        fn from(err: GetProductError) -> Self {
            match err {
                GetProductError::ProductNotFound(_, _) => {
                    ApiError::not_found(PRODUCT_NOT_FOUND, Box::new(err))
                }
                GetProductError::MonetaryAmountOverflowError(_) => {
                    ApiError::internal_server_error(MONETARY_AMOUNT_OVERFLOW, Box::new(err))
                }
                GetProductError::SdkGetItemError(err) => err.into(),
                GetProductError::SdkBatchGetItemError(err) => err.into(),
                GetProductError::SdkQueryError(err) => err.into(),
                GetProductError::UnprocessedAfterMaxRetries(_) => {
                    ApiError::service_unavailable(UNPROCESSED_AFTER_MAX_RETRIES, Box::new(err))
                }
            }
        }
    }
}

#[async_trait]
#[mockall::automock]
pub trait GetProductService {
    async fn find_product(
        &self,
        shop_id: &ShopId,
        shops_product_id: &ShopsProductId,
    ) -> Result<Product, GetProductError>;

    async fn view_product(
        &self,
        shop_id: &ShopId,
        shops_product_id: &ShopsProductId,
        languages: &[Language],
        currency: &Currency,
        history: bool,
    ) -> Result<LocalizedProductView, GetProductError>;

    async fn view_products(
        &self,
        items: Vec<ProductKey>,
        languages: &[Language],
        currency: &Currency,
    ) -> Result<Vec<LocalizedProductView>, GetProductError>;
}

pub struct GetProductServiceImpl<'a> {
    repository: &'a (dyn ProductDynamoDbRepository + Sync),
}

impl<'a> GetProductServiceImpl<'a> {
    pub fn new(repository: &'a (dyn ProductDynamoDbRepository + Sync)) -> Self {
        Self { repository }
    }
}

#[async_trait]
impl<'a> GetProductService for GetProductServiceImpl<'a> {
    async fn find_product(
        &self,
        shop_id: &ShopId,
        shops_product_id: &ShopsProductId,
    ) -> Result<Product, GetProductError> {
        let product_record = self
            .repository
            .get_product_record(shop_id, shops_product_id)
            .await?
            .ok_or(GetProductError::ProductNotFound(
                *shop_id,
                shops_product_id.clone(),
            ))?;

        Ok(product_record.into())
    }

    async fn view_product(
        &self,
        shop_id: &ShopId,
        shops_product_id: &ShopsProductId,
        preferred_languages: &[Language],
        currency: &Currency,
        history: bool,
    ) -> Result<LocalizedProductView, GetProductError> {
        let (product_record, event_records) = if history {
            self.repository
                .query_product_record_and_event_records(shop_id, shops_product_id)
                .await?
                .ok_or(GetProductError::ProductNotFound(
                    *shop_id,
                    shops_product_id.clone(),
                ))?
        } else {
            let product_record = self
                .repository
                .get_product_record(shop_id, shops_product_id)
                .await?
                .ok_or(GetProductError::ProductNotFound(
                    *shop_id,
                    shops_product_id.clone(),
                ))?;
            (product_record, vec![])
        };

        let mut product_view =
            localize_product_record(product_record, currency, preferred_languages);

        let event_views = event_records
            .into_iter()
            .map(ProductEvent::try_from)
            .filter_map(|event_res| match event_res {
                Ok(event) => Some(event),
                Err(err) => {
                    error!(
                        error = %err,
                        fromType = %std::any::type_name::<ProductEventRecord>(),
                        toType = %std::any::type_name::<ProductEvent>(),
                        "Failed mapping types."
                    );
                    None
                }
            })
            .map(|event| localize_product_event(event, currency))
            .collect();
        product_view.history = if history { Some(event_views) } else { None };

        Ok(product_view)
    }

    async fn view_products(
        &self,
        items: Vec<ProductKey>,
        languages: &[Language],
        currency: &Currency,
    ) -> Result<Vec<LocalizedProductView>, GetProductError> {
        const MAX_RETRIES: u32 = 3;
        const BASE_DELAY_MS: u64 = 100;

        let mut views = Vec::with_capacity(items.len());
        let mut unprocessed = items;
        let mut retry_count = 0;
        loop {
            let (mut local_views, local_unprocessed) = self
                .view_products_with_unprocessed(unprocessed, languages, currency)
                .await?;
            views.append(&mut local_views);

            if local_unprocessed.is_empty() {
                break;
            } else if retry_count >= MAX_RETRIES {
                return Err(GetProductError::UnprocessedAfterMaxRetries(MAX_RETRIES));
            }

            retry_count += 1;
            let delay_ms = BASE_DELAY_MS * 2_u64.pow(retry_count - 1);
            tokio::time::sleep(tokio::time::Duration::from_millis(delay_ms)).await;

            unprocessed = local_unprocessed;
        }

        Ok(views)
    }
}

impl<'a> GetProductServiceImpl<'a> {
    async fn view_products_with_unprocessed(
        &self,
        items: Vec<ProductKey>,
        languages: &[Language],
        currency: &Currency,
    ) -> Result<(Vec<LocalizedProductView>, Vec<ProductKey>), GetProductError> {
        let mut views = Vec::with_capacity(items.len());
        let mut unprocessed = Vec::new();
        for batch in Batch::chunked_from(items.into_iter()) {
            let result = self.repository.get_product_records(&batch).await?;
            if let Some(up) = result.unprocessed {
                unprocessed.extend(up);
            }
            let local_views = result
                .items
                .into_iter()
                .map(|record| localize_product_record(record, currency, languages));
            views.extend(local_views);
        }

        Ok((views, unprocessed))
    }
}

fn localize_product_record(
    product_record: ProductRecord,
    currency: &Currency,
    preferred_languages: &[Language],
) -> LocalizedProductView {
    let mut available_titles: HashMap<Language, Title> = HashMap::with_capacity(Language::COUNT);
    available_titles.insert(
        product_record.title_native.language.into(),
        product_record.title_native.text.into(),
    );
    if let Some(title_de) = product_record.title_de {
        available_titles.insert(Language::De, title_de.into());
    }
    if let Some(title_en) = product_record.title_en {
        available_titles.insert(Language::En, title_en.into());
    }
    if let Some(title_fr) = product_record.title_fr {
        available_titles.insert(Language::Fr, title_fr.into());
    }
    if let Some(title_es) = product_record.title_es {
        available_titles.insert(Language::Es, title_es.into());
    }

    let mut available_descriptions: HashMap<Language, Description> =
        HashMap::with_capacity(Language::COUNT);
    if let Some(description_native) = product_record.description_native {
        available_descriptions.insert(
            description_native.language.into(),
            description_native.text.into(),
        );
    }
    if let Some(description_de) = product_record.description_de {
        available_descriptions.insert(Language::De, description_de.into());
    }
    if let Some(description_en) = product_record.description_en {
        available_descriptions.insert(Language::En, description_en.into());
    }
    if let Some(description_fr) = product_record.description_fr {
        available_descriptions.insert(Language::Fr, description_fr.into());
    }
    if let Some(description_es) = product_record.description_es {
        available_descriptions.insert(Language::Es, description_es.into());
    }

    let title = Language::resolve(preferred_languages, available_titles).unwrap_or_else(|| {
        error!("Failed resolving title. This SHOULD be impossible because the native title always exists.");
        Localized::new(Language::En, "Unknown title".into())
    });
    let description = Language::resolve(preferred_languages, available_descriptions);

    let price = match currency {
        Currency::Eur => product_record
            .price_eur
            .map(|amount| Price::new(amount.into(), Currency::Eur)),
        Currency::Gbp => product_record
            .price_gbp
            .map(|amount| Price::new(amount.into(), Currency::Gbp)),
        Currency::Usd => product_record
            .price_usd
            .map(|amount| Price::new(amount.into(), Currency::Usd)),
        Currency::Aud => product_record
            .price_aud
            .map(|amount| Price::new(amount.into(), Currency::Aud)),
        Currency::Cad => product_record
            .price_cad
            .map(|amount| Price::new(amount.into(), Currency::Cad)),
        Currency::Nzd => product_record
            .price_nzd
            .map(|amount| Price::new(amount.into(), Currency::Nzd)),
    };

    LocalizedProductView {
        product_id: product_record.product_id,
        event_id: product_record.event_id,
        shop_id: product_record.shop_id,
        shops_product_id: product_record.shops_product_id,
        shop_name: product_record.shop_name.into(),
        title,
        description,
        price,
        state: product_record.state.into(),
        url: product_record.url,
        images: product_record.images,
        created: product_record.created,
        updated: product_record.updated,
        history: None,
    }
}

fn localize_product_event(
    event: ProductEvent,
    currency: &Currency,
) -> Event<ProductId, LocalizedProductEventPayloadView> {
    let payload = match event.payload {
        ProductEventPayload::Created(payload) => {
            let mut prices = payload.other_price;
            if let Some(native_price) = payload.native_price {
                prices.insert(native_price.currency, native_price.monetary_amount);
            }
            LocalizedProductEventPayloadView::Created(LocalizedProductCreatedEventPayloadView {
                shop_id: payload.shop_id,
                shops_product_id: payload.shops_product_id,
                shop_name: payload.shop_name,
                title: payload.native_title,
                description: payload.native_description,
                price: prices
                    .remove(currency)
                    .map(|amount| Price::new(amount, *currency)),
                state: payload.state,
                url: payload.url,
                images: payload.images,
            })
        }
        ProductEventPayload::StateListed(payload) => LocalizedProductEventPayloadView::StateListed(
            LocalizedProductStateChangeEventPayloadView {
                shop_id: payload.shop_id,
                shops_product_id: payload.shops_product_id,
                old_state: payload.old_state,
            },
        ),
        ProductEventPayload::StateAvailable(payload) => {
            LocalizedProductEventPayloadView::StateAvailable(
                LocalizedProductStateChangeEventPayloadView {
                    shop_id: payload.shop_id,
                    shops_product_id: payload.shops_product_id,
                    old_state: payload.old_state,
                },
            )
        }
        ProductEventPayload::StateReserved(payload) => {
            LocalizedProductEventPayloadView::StateReserved(
                LocalizedProductStateChangeEventPayloadView {
                    shop_id: payload.shop_id,
                    shops_product_id: payload.shops_product_id,
                    old_state: payload.old_state,
                },
            )
        }
        ProductEventPayload::StateSold(payload) => LocalizedProductEventPayloadView::StateSold(
            LocalizedProductStateChangeEventPayloadView {
                shop_id: payload.shop_id,
                shops_product_id: payload.shops_product_id,
                old_state: payload.old_state,
            },
        ),
        ProductEventPayload::StateRemoved(payload) => {
            LocalizedProductEventPayloadView::StateRemoved(
                LocalizedProductStateChangeEventPayloadView {
                    shop_id: payload.shop_id,
                    shops_product_id: payload.shops_product_id,
                    old_state: payload.old_state,
                },
            )
        }
        ProductEventPayload::StateUnknown(payload) => {
            LocalizedProductEventPayloadView::StateUnknown(
                LocalizedProductStateChangeEventPayloadView {
                    shop_id: payload.shop_id,
                    shops_product_id: payload.shops_product_id,
                    old_state: payload.old_state,
                },
            )
        }
        ProductEventPayload::PriceDiscovered(payload) => {
            let mut prices = payload.other_price;
            prices.insert(
                payload.native_price.currency,
                payload.native_price.monetary_amount,
            );
            LocalizedProductEventPayloadView::PriceDiscovered(
                LocalizedProductPriceDiscoveryEventPayloadView {
                        shop_id: payload.shop_id,
                        shops_product_id: payload.shops_product_id,
                        price: Currency::resolve(&[*currency], prices).unwrap_or_else(|| {
                            error!("Failed resolving price. This SHOULD be impossible because the native price always exists.");
                            Price::new(0u64.into(), *currency)
                        }),
                    },
                )
        }
        ProductEventPayload::PriceDropped(payload) => {
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
            LocalizedProductEventPayloadView::PriceDropped(
                LocalizedProductPriceChangeEventPayloadView {
                        shop_id: payload.shop_id,
                        shops_product_id: payload.shops_product_id,
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
        ProductEventPayload::PriceIncreased(payload) => {
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
            LocalizedProductEventPayloadView::PriceIncreased(
                LocalizedProductPriceChangeEventPayloadView {
                        shop_id: payload.shop_id,
                        shops_product_id: payload.shops_product_id,
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
        ProductEventPayload::PriceRemoved(payload) => {
            let mut old_prices = payload.old_other_price;
            old_prices.insert(
                payload.old_native_price.currency,
                payload.old_native_price.monetary_amount,
            );
            LocalizedProductEventPayloadView::PriceRemoved(LocalizedProductPriceRemovedEventPayloadView {
                shop_id: payload.shop_id,
                shops_product_id: payload.shops_product_id,
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
    mod find_product {
        use crate::dynamodb::repository::MockProductDynamoDbRepository;
        use crate::service::get_service::{
            GetProductError, GetProductService, GetProductServiceImpl,
        };
        use aws_sdk_dynamodb::{
            config::http::HttpResponse,
            error::{ConnectorError, SdkError},
        };
        use common::{shop_id::ShopId, shops_product_id::ShopsProductId};
        use fake::{Fake, Faker};

        #[tokio::test]
        async fn should_return_product_when_exists() {
            let mut repository = MockProductDynamoDbRepository::default();
            repository
                .expect_get_product_record()
                .return_once(|_, _| Box::pin(async { Ok(Some(Faker.fake())) }));
            let service = GetProductServiceImpl {
                repository: &repository,
            };
            let actual = service
                .find_product(&ShopId::new(), &ShopsProductId::new())
                .await;
            assert!(actual.is_ok());
        }

        #[tokio::test]
        async fn should_return_product_not_found_err_when_product_does_not_exist() {
            let shop_id = ShopId::new();
            let shops_product_id = "non-existent".into();
            let mut repository = MockProductDynamoDbRepository::default();
            repository
                .expect_get_product_record()
                .return_once(|_, _| Box::pin(async { Ok(None) }));
            let service = GetProductServiceImpl {
                repository: &repository,
            };
            let actual = service.find_product(&shop_id, &shops_product_id).await;

            assert!(actual.is_err());
            match actual.unwrap_err() {
                GetProductError::ProductNotFound(err_shop_id, err_shops_product_id) => {
                    assert_eq!(err_shop_id, shop_id);
                    assert_eq!(err_shops_product_id, shops_product_id);
                }
                _ => panic!("expected GetProductError::ProductNotFound"),
            }
        }

        #[tokio::test]
        #[rstest::rstest]
        #[trace]
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
            let shops_product_id = "non-existent".into();
            let mut repository = MockProductDynamoDbRepository::default();
            repository
                .expect_get_product_record()
                .return_once(|_, _| Box::pin(async { Err(expected) }));
            let service = GetProductServiceImpl {
                repository: &repository,
            };
            let actual = service.find_product(&shop_id, &shops_product_id).await;

            assert!(actual.is_err());
            match actual.unwrap_err() {
                GetProductError::SdkGetItemError(_) => {}
                _ => panic!("expected GetProductError::ProductNotFound"),
            }
        }
    }

    mod view_product {
        use crate::dynamodb::{
            product_event_record::ProductEventRecord, product_record::ProductRecord,
            repository::MockProductDynamoDbRepository,
        };
        use crate::service::get_service::{
            GetProductError, GetProductService, GetProductServiceImpl,
        };
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
            shops_product_id::ShopsProductId,
        };
        use fake::{Fake, Faker};
        use itertools::Itertools;

        #[tokio::test]
        async fn should_return_product_when_exists_without_history() {
            let mut repository = MockProductDynamoDbRepository::default();
            repository
                .expect_get_product_record()
                .return_once(|_, _| Box::pin(async { Ok(Some(Faker.fake())) }));
            let service = GetProductServiceImpl {
                repository: &repository,
            };
            let actual = service
                .view_product(
                    &ShopId::new(),
                    &ShopsProductId::new(),
                    &[],
                    &Currency::Eur,
                    false,
                )
                .await;
            assert!(actual.is_ok());
        }

        #[tokio::test]
        async fn should_return_product_when_exists_with_history() {
            let mut repository = MockProductDynamoDbRepository::default();
            repository
                .expect_query_product_record_and_event_records()
                .return_once(|_, _| Box::pin(async { Ok(Some(Faker.fake())) }));
            let service = GetProductServiceImpl {
                repository: &repository,
            };
            let actual = service
                .view_product(
                    &ShopId::new(),
                    &ShopsProductId::new(),
                    &[],
                    &Currency::Eur,
                    true,
                )
                .await;
            assert!(actual.is_ok());
        }

        #[tokio::test]
        async fn should_keep_history_in_exact_order_as_dynamodb_read() {
            let mut repository = MockProductDynamoDbRepository::default();
            let mut events = fake::vec![ProductEventRecord; 100];
            events.sort_by(|l, r| l.product_id.cmp(&r.product_id));
            let expected = events
                .clone()
                .into_iter()
                .map(|record| record.product_id)
                .collect_vec();
            repository
                .expect_query_product_record_and_event_records()
                .return_once(|_, _| Box::pin(async { Ok(Some((Faker.fake(), events))) }));
            let service = GetProductServiceImpl {
                repository: &repository,
            };
            let actual = service
                .view_product(
                    &ShopId::new(),
                    &ShopsProductId::new(),
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
        #[trace]
        #[case::eur(Currency::Eur, 2)]
        #[case::gbp(Currency::Gbp, 4)]
        #[case::usd(Currency::Usd, 10)]
        #[case::aud(Currency::Aud, 1000)]
        #[case::cad(Currency::Cad, 4000)]
        #[case::nzd(Currency::Nzd, 42)]
        async fn should_respect_currency(#[case] currency: Currency, #[case] expected_amount: u64) {
            let mut repository = MockProductDynamoDbRepository::default();
            let mut expected_record: ProductRecord = Faker.fake();
            expected_record.price_eur = Some(2);
            expected_record.price_gbp = Some(4);
            expected_record.price_usd = Some(10);
            expected_record.price_aud = Some(1000);
            expected_record.price_cad = Some(4000);
            expected_record.price_nzd = Some(42);
            repository
                .expect_get_product_record()
                .return_once(|_, _| Box::pin(async { Ok(Some(expected_record)) }));
            let service = GetProductServiceImpl {
                repository: &repository,
            };
            let actual_price = service
                .view_product(
                    &ShopId::new(),
                    &ShopsProductId::new(),
                    &[],
                    &currency,
                    false,
                )
                .await
                .unwrap()
                .price
                .unwrap();
            assert_eq!(currency, actual_price.currency);
            assert_eq!(expected_amount, u64::from(actual_price.monetary_amount));
        }

        #[tokio::test]
        #[rstest::rstest]
        #[trace]
        #[case(&[], De, "German")]
        #[case(&[De], De, "German")]
        #[case(&[De, En], De, "German")]
        #[case(&[De, Fr], De, "German")]
        #[case(&[Fr, De, En, Es], Fr, "French")]
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
            let mut repository = MockProductDynamoDbRepository::default();
            let mut expected_record: ProductRecord = Faker.fake();
            expected_record.title_native = TextRecord::new("Spanish", LanguageRecord::Es);
            expected_record.title_de = Some("German".to_string());
            expected_record.title_en = Some("English".to_string());
            expected_record.title_fr = Some("French".to_string());
            expected_record.title_es = Some("Spanish".to_string());
            repository
                .expect_get_product_record()
                .return_once(|_, _| Box::pin(async { Ok(Some(expected_record)) }));
            let service = GetProductServiceImpl {
                repository: &repository,
            };
            let actual_title = service
                .view_product(
                    &ShopId::new(),
                    &ShopsProductId::new(),
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
        #[trace]
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
            let mut repository = MockProductDynamoDbRepository::default();
            let mut expected_record: ProductRecord = Faker.fake();
            expected_record.title_native = TextRecord::new("Spanish", LanguageRecord::Es);
            expected_record.title_de = None;
            expected_record.title_en = None;
            expected_record.title_fr = None;
            expected_record.title_es = None;
            repository
                .expect_get_product_record()
                .return_once(|_, _| Box::pin(async { Ok(Some(expected_record)) }));
            let service = GetProductServiceImpl {
                repository: &repository,
            };
            let actual_title = service
                .view_product(
                    &ShopId::new(),
                    &ShopsProductId::new(),
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
        #[trace]
        #[case(&[], De, "German")]
        #[case(&[De], De, "German")]
        #[case(&[De, En], De, "German")]
        #[case(&[De, Fr], De, "German")]
        #[case(&[Fr, De, En, Es], Fr, "French")]
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
            let mut repository = MockProductDynamoDbRepository::default();
            let mut expected_record: ProductRecord = Faker.fake();
            expected_record.description_native =
                Some(TextRecord::new("Spanish", LanguageRecord::Es));
            expected_record.description_de = Some("German".to_string());
            expected_record.description_en = Some("English".to_string());
            expected_record.description_fr = Some("French".to_string());
            expected_record.description_es = Some("Spanish".to_string());
            repository
                .expect_get_product_record()
                .return_once(|_, _| Box::pin(async { Ok(Some(expected_record)) }));
            let service = GetProductServiceImpl {
                repository: &repository,
            };
            let actual_description = service
                .view_product(
                    &ShopId::new(),
                    &ShopsProductId::new(),
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
        #[trace]
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
            let mut repository = MockProductDynamoDbRepository::default();
            let mut expected_record: ProductRecord = Faker.fake();
            expected_record.description_native =
                Some(TextRecord::new("Spanish", LanguageRecord::Es));
            expected_record.description_de = None;
            expected_record.description_en = None;
            expected_record.description_fr = None;
            expected_record.description_es = None;
            repository
                .expect_get_product_record()
                .return_once(|_, _| Box::pin(async { Ok(Some(expected_record)) }));
            let service = GetProductServiceImpl {
                repository: &repository,
            };
            let actual_description = service
                .view_product(
                    &ShopId::new(),
                    &ShopsProductId::new(),
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
        #[trace]
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
        async fn should_return_product_without_description_when_none_exists(
            #[case] languages: &[Language],
        ) {
            let mut repository = MockProductDynamoDbRepository::default();
            let mut expected_record: ProductRecord = Faker.fake();
            expected_record.description_native = None;
            expected_record.description_de = None;
            expected_record.description_en = None;
            expected_record.description_fr = None;
            expected_record.description_es = None;
            repository
                .expect_get_product_record()
                .return_once(|_, _| Box::pin(async { Ok(Some(expected_record)) }));
            let service = GetProductServiceImpl {
                repository: &repository,
            };
            let actual_description = service
                .view_product(
                    &ShopId::new(),
                    &ShopsProductId::new(),
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
        async fn should_return_product_not_found_err_when_product_does_not_exist() {
            let shop_id = ShopId::new();
            let shops_product_id = "non-existent".into();
            let mut repository = MockProductDynamoDbRepository::default();
            repository
                .expect_get_product_record()
                .return_once(|_, _| Box::pin(async { Ok(None) }));
            let service = GetProductServiceImpl {
                repository: &repository,
            };
            let actual = service
                .view_product(&shop_id, &shops_product_id, &[], &Currency::Eur, false)
                .await;

            assert!(actual.is_err());
            match actual.unwrap_err() {
                GetProductError::ProductNotFound(err_shop_id, err_shops_product_id) => {
                    assert_eq!(err_shop_id, shop_id);
                    assert_eq!(err_shops_product_id, shops_product_id);
                }
                _ => panic!("expected GetProductError::ProductNotFound"),
            }
        }

        #[tokio::test]
        #[rstest::rstest]
        #[trace]
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
            let shops_product_id = "non-existent".into();
            let mut repository = MockProductDynamoDbRepository::default();
            repository
                .expect_get_product_record()
                .return_once(|_, _| Box::pin(async { Err(expected) }));
            let service = GetProductServiceImpl {
                repository: &repository,
            };
            let actual = service
                .view_product(&shop_id, &shops_product_id, &[], &Currency::Eur, false)
                .await;

            assert!(actual.is_err());
            match actual.unwrap_err() {
                GetProductError::SdkGetItemError(_) => {}
                _ => panic!("expected GetProductError::ProductNotFound"),
            }
        }
    }

    mod view_products {
        use crate::dynamodb::{
            product_record::ProductRecord, repository::MockProductDynamoDbRepository,
        };
        use crate::service::get_service::{
            GetProductError, GetProductService, GetProductServiceImpl,
        };
        use aws_sdk_dynamodb::{
            config::http::HttpResponse,
            error::{ConnectorError, SdkError},
        };
        use common::{batch::Batch, currency::domain::Currency};
        use common::{batch::dynamodb::BatchGetItemResult, product_id::ProductKey};

        #[tokio::test]
        async fn should_return_products_when_all_processed() {
            let mut repository = MockProductDynamoDbRepository::default();
            repository.expect_get_product_records().returning(|_| {
                Box::pin(async {
                    Ok(BatchGetItemResult {
                        items: fake::vec![ProductRecord; 42],
                        unprocessed: None,
                    })
                })
            });
            let service = GetProductServiceImpl {
                repository: &repository,
            };
            let actual = service
                .view_products(fake::vec![ProductKey; 42], &[], &Currency::Eur)
                .await
                .unwrap();
            assert_eq!(42, actual.len());
        }

        #[tokio::test]
        async fn should_completely_fail_when_some_processed() {
            let mut repository = MockProductDynamoDbRepository::default();
            repository.expect_get_product_records().returning(|_| {
                Box::pin(async {
                    Ok(BatchGetItemResult {
                        items: fake::vec![ProductRecord; 37],
                        unprocessed: Some(
                            Batch::try_from_iter(fake::vec![ProductKey; 5].into_iter()).unwrap(),
                        ),
                    })
                })
            });
            let service = GetProductServiceImpl {
                repository: &repository,
            };
            let actual = service
                .view_products(fake::vec![ProductKey; 42], &[], &Currency::Eur)
                .await
                .unwrap_err();
            match actual {
                GetProductError::UnprocessedAfterMaxRetries(_) => {}
                other => {
                    panic!(
                        "Expected 'GetProductError::UnprocessedAfterMaxRetries'. Got '{other:?}'."
                    )
                }
            }
        }

        #[tokio::test]
        #[rstest::rstest]
        #[trace]
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
            let mut repository = MockProductDynamoDbRepository::default();
            repository
                .expect_get_product_records()
                .return_once(|_| Box::pin(async { Err(expected) }));
            let service = GetProductServiceImpl {
                repository: &repository,
            };
            let actual = service
                .view_products(fake::vec![ProductKey; 222], &[], &Currency::Eur)
                .await;

            assert!(actual.is_err());
            match actual.unwrap_err() {
                GetProductError::SdkBatchGetItemError(_) => {}
                _ => panic!("expected GetProductError::SdkBatchGetItemError"),
            }
        }
    }
}

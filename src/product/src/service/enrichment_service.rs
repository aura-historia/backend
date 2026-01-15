use crate::service::product_command::PipedProductCommand;
use common::{
    batch::Batch,
    domain::{Domain, NoDomainError},
    price::domain::{FxRate, MonetaryAmountOverflowError},
    shop_id::ShopIdentifier,
};
use shop::core::shop::Shop;
use shop::dynamodb::repository::ShopDynamoDbRepository;
use std::collections::{HashMap, HashSet};
use tracing::{error, warn};

#[derive(Debug, Clone, PartialEq, Default)]
pub struct EnrichProductCommandsOutput {
    pub enriched: Vec<PipedProductCommand>,
    pub failed: Vec<(PipedProductCommand, EnrichProductCommandError)>,
    pub unprocessed: Vec<PipedProductCommand>,
}

#[derive(thiserror::Error, Debug, Clone, PartialEq, Eq)]
pub enum EnrichProductCommandError {
    #[error("MonetaryAmountOverflowError: {0}")]
    MonetaryAmountOverflowError(#[from] MonetaryAmountOverflowError),

    #[error("No Shop with domain '{0}' exists. Cannot enrich shop-information.")]
    UnknownShopDomain(Domain),

    #[error("NoShopDomain: {0}")]
    NoShopDomain(#[from] NoDomainError),
}

#[async_trait::async_trait]
#[mockall::automock]
pub trait ProductCommandEnrichmentService {
    async fn enrich_shop(&self, commands: Vec<PipedProductCommand>) -> EnrichProductCommandsOutput;

    fn enrich_price(&self, commands: Vec<PipedProductCommand>) -> EnrichProductCommandsOutput;

    async fn enrich(&self, commands: Vec<PipedProductCommand>) -> EnrichProductCommandsOutput {
        let mut price_enriched_res = self.enrich_price(commands);
        let mut shop_enriched_res = self.enrich_shop(price_enriched_res.enriched).await;

        shop_enriched_res
            .unprocessed
            .append(&mut price_enriched_res.unprocessed);
        shop_enriched_res
            .failed
            .append(&mut price_enriched_res.failed);

        shop_enriched_res
    }
}

pub struct ProductCommandEnrichmentServiceImpl<'a, T: FxRate + Sync> {
    shop_dynamodb_repository: &'a (dyn ShopDynamoDbRepository + Sync),
    fx_rate: &'a T,
}

impl<'a, T: FxRate + Sync> ProductCommandEnrichmentServiceImpl<'a, T> {
    pub fn new(
        shop_dynamodb_repository: &'a (dyn ShopDynamoDbRepository + Sync),
        fx_rate: &'a T,
    ) -> Self {
        Self {
            shop_dynamodb_repository,
            fx_rate,
        }
    }
}

#[async_trait::async_trait]
impl<'a, T: FxRate + Sync> ProductCommandEnrichmentService
    for ProductCommandEnrichmentServiceImpl<'a, T>
{
    async fn enrich_shop(&self, commands: Vec<PipedProductCommand>) -> EnrichProductCommandsOutput {
        let shop_identifiers = commands
            .iter()
            .map(|cmd| cmd.url.clone())
            .filter_map(|url |{
                match Domain::try_from(&url) {
                    Ok(domain) => Some(domain),
                    Err(err) => {
                        warn!(error = %err, url = %url, "Cannot extract domain from Product-URL. Skipping product for Shop-Enrichment.");
                        None
                    },
                }
            })
            .map(ShopIdentifier::from)
            .collect::<HashSet<_>>();
        let mut shops: HashMap<Domain, Shop> = HashMap::with_capacity(shop_identifiers.len());
        let mut unprocessed_shops = HashSet::new();

        for batch in Batch::chunked_from(shop_identifiers.into_iter()) {
            match self.shop_dynamodb_repository.get_shop_records(&batch).await {
                Ok(res) => {
                    if let Some(unprocessed) = res.unprocessed {
                        unprocessed_shops.extend(&mut unprocessed.into_iter());
                    }
                    for record in res.items {
                        for domain in record.domains.clone() {
                            shops.insert(domain, record.clone().into());
                        }
                    }
                }
                Err(err) => {
                    error!(error = ?err, "Failed entire BatchGetItem-Operation when getting shops.");
                    unprocessed_shops.extend(batch);
                }
            };
        }

        let mut output = EnrichProductCommandsOutput::default();
        for mut cmd in commands {
            match Domain::try_from(&cmd.url) {
                Ok(domain) => {
                    let shop_identifier = ShopIdentifier::ShopDomain(domain.clone());
                    if unprocessed_shops.contains(&shop_identifier) {
                        output.unprocessed.push(cmd);
                    } else if cmd.shop_id.is_none()
                        || cmd.shop_name.is_none()
                        || cmd.shop_type.is_none()
                    {
                        match shops.get(&domain) {
                            Some(shop) => {
                                cmd.shop_id = Some(shop.shop_id);
                                cmd.shop_name = Some(shop.name.clone());
                                cmd.shop_type = Some(shop.shop_type);
                                output.enriched.push(cmd);
                            }
                            None => output
                                .failed
                                .push((cmd, EnrichProductCommandError::UnknownShopDomain(domain))),
                        }
                    }
                }
                Err(err) => {
                    output
                        .failed
                        .push((cmd, EnrichProductCommandError::NoShopDomain(err)));
                }
            }
        }

        output
    }

    fn enrich_price(&self, commands: Vec<PipedProductCommand>) -> EnrichProductCommandsOutput {
        let mut output = EnrichProductCommandsOutput::default();

        for mut cmd in commands {
            let other_price_res = cmd
                .native_price
                .as_ref()
                .map(|price| {
                    self.fx_rate
                        .exchange_all(price.currency, price.monetary_amount)
                })
                .unwrap_or_else(|| Ok(HashMap::default()));

            let other_price_estimate_min_res = cmd
                .native_price_estimate_min
                .as_ref()
                .map(|price| {
                    self.fx_rate
                        .exchange_all(price.currency, price.monetary_amount)
                })
                .unwrap_or_else(|| Ok(HashMap::default()));

            let other_price_estimate_max_res = cmd
                .native_price_estimate_max
                .as_ref()
                .map(|price| {
                    self.fx_rate
                        .exchange_all(price.currency, price.monetary_amount)
                })
                .unwrap_or_else(|| Ok(HashMap::default()));

            match (other_price_res, other_price_estimate_min_res, other_price_estimate_max_res) {
                (Ok(other_price), Ok(other_price_estimate_min), Ok(other_price_estimate_max)) => {
                    cmd.other_price = other_price;
                    cmd.other_price_estimate_min = other_price_estimate_min;
                    cmd.other_price_estimate_max = other_price_estimate_max;
                    output.enriched.push(cmd);
                }
                (Err(err), _, _) | (_, Err(err), _) | (_, _, Err(err)) => {
                    output
                        .failed
                        .push((cmd, EnrichProductCommandError::from(err)));
                }
            }
        }

        output
    }
}

#[cfg(feature = "test-data")]
mod faker {
    use super::*;
    use common::price::domain::{FixedFxRate, Price};
    use fake::{Dummy, Fake, Faker, Rng};
    use time::OffsetDateTime;
    use url::Url;

    impl Dummy<Faker> for PipedProductCommand {
        fn dummy_with_rng<R: Rng + ?Sized>(config: &Faker, rng: &mut R) -> Self {
            let native_price: Option<Price> = config.fake_with_rng(rng);
            let other_price = match native_price {
                None => HashMap::new(),
                Some(price) => FixedFxRate()
                    .exchange_all(price.currency, price.monetary_amount)
                    .unwrap(),
            };
            let native_price_estimate_min: Option<Price> = config.fake_with_rng(rng);
            let other_price_estimate_min = match native_price_estimate_min {
                None => HashMap::new(),
                Some(price) => FixedFxRate()
                    .exchange_all(price.currency, price.monetary_amount)
                    .unwrap(),
            };
            let native_price_estimate_max: Option<Price> = config.fake_with_rng(rng);
            let other_price_estimate_max = match native_price_estimate_max {
                None => HashMap::new(),
                Some(price) => FixedFxRate()
                    .exchange_all(price.currency, price.monetary_amount)
                    .unwrap(),
            };
            let state = config.fake_with_rng(rng);
            PipedProductCommand {
                shop_id: config.fake_with_rng(rng),
                shops_product_id: config.fake_with_rng(rng),
                shop_name: config.fake_with_rng(rng),
                shop_type: config.fake_with_rng(rng),
                native_title: config.fake_with_rng(rng),
                other_title: config.fake_with_rng(rng),
                native_description: config.fake_with_rng(rng),
                other_description: config.fake_with_rng(rng),
                native_price,
                other_price,
                native_price_estimate_min,
                other_price_estimate_min,
                native_price_estimate_max,
                other_price_estimate_max,
                state,
                url: Url::parse(&format!(
                    "https://foo.bar/item/{}",
                    config.fake_with_rng::<u16, _>(rng)
                ))
                .unwrap(),
                images: Faker.fake(),
                auction_start: if config.fake_with_rng(rng) {
                    Some(OffsetDateTime::now_utc())
                } else {
                    None
                },
                auction_end: if config.fake_with_rng(rng) {
                    Some(OffsetDateTime::now_utc())
                } else {
                    None
                },
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::service::enrichment_service::{
        PipedProductCommand, ProductCommandEnrichmentService, ProductCommandEnrichmentServiceImpl,
    };
    use aws_sdk_sqs::error::SdkError;
    use common::{
        batch::dynamodb::BatchGetItemResult, currency::domain::Currency,
        price::domain::FixedFxRate, shop_id::ShopIdentifier,
    };
    use fake::{Fake, Faker};
    use shop::core::shop::Shop;
    use shop::dynamodb::{repository::MockShopDynamoDbRepository, shop_record::ShopRecord};
    use std::panic;
    use strum::EnumCount;

    #[test]
    fn should_enrich_price_when_other_none() {
        let repository = MockShopDynamoDbRepository::new();
        let fx_rate = FixedFxRate();
        let service = ProductCommandEnrichmentServiceImpl::new(&repository, &fx_rate);

        let mut cmd = Faker.fake::<PipedProductCommand>();
        cmd.native_price = Some(Faker.fake());
        cmd.other_price.clear();

        let actual = service.enrich_price(vec![cmd]);

        assert_eq!(1, actual.enriched.len());
        assert_eq!(Currency::COUNT, actual.enriched[0].other_price.len());
    }

    #[tokio::test]
    async fn should_return_enriched_products() {
        let mut repository = MockShopDynamoDbRepository::new();
        let fx_rate = FixedFxRate();

        repository.expect_get_shop_records().returning(|batch| {
            let batch_clone = batch.clone();
            Box::pin(async {
                Ok(BatchGetItemResult {
                    items: batch_clone
                        .into_iter()
                        .flat_map(|shop_identifier| {
                            let mut shop = Faker.fake::<Shop>();
                            let url = match shop_identifier {
                                ShopIdentifier::ShopId(_) => {
                                    panic!("Expected 'ShopIdentifier::ShopUrl'")
                                }
                                ShopIdentifier::ShopDomain(url) => url,
                            };
                            shop.domains.insert(url);
                            ShopRecord::clone_from_shop_as_shop_domain_records(&shop)
                        })
                        .collect(),
                    unprocessed: None,
                })
            })
        });

        let service = ProductCommandEnrichmentServiceImpl::new(&repository, &fx_rate);
        let cmds = fake::vec![PipedProductCommand; 1234];
        let actual = service.enrich_shop(cmds.clone()).await;

        assert!(actual.failed.is_empty());
        assert!(actual.unprocessed.is_empty());
        assert_eq!(
            cmds.into_iter()
                .filter(|piped_cmd| {
                    piped_cmd.shop_id.is_none()
                        || piped_cmd.shop_name.is_none()
                        || piped_cmd.shop_type.is_none()
                })
                .count(),
            actual.enriched.len()
        );
    }

    #[tokio::test]
    async fn should_return_unprocessed_products_for_unprocessed_shops() {
        let mut repository = MockShopDynamoDbRepository::new();
        let fx_rate = FixedFxRate();

        repository.expect_get_shop_records().returning(|batch| {
            let batch_clone = batch.clone();
            Box::pin(async {
                Ok(BatchGetItemResult {
                    items: Faker.fake(),
                    unprocessed: Some(batch_clone),
                })
            })
        });

        let service = ProductCommandEnrichmentServiceImpl::new(&repository, &fx_rate);
        let actual = service
            .enrich_shop(fake::vec![PipedProductCommand; 1234])
            .await;

        assert!(actual.failed.is_empty());
        assert!(actual.enriched.is_empty());
        assert_eq!(1234, actual.unprocessed.len());
    }

    #[tokio::test]
    async fn should_return_unprocessed_products_for_failed_shops() {
        let mut repository = MockShopDynamoDbRepository::new();
        let fx_rate = FixedFxRate();

        repository.expect_get_shop_records().returning(|_| {
            Box::pin(async { Err(SdkError::construction_failure("Something went wrong")) })
        });

        let service = ProductCommandEnrichmentServiceImpl::new(&repository, &fx_rate);
        let actual = service
            .enrich_shop(fake::vec![PipedProductCommand; 1234])
            .await;

        assert!(actual.failed.is_empty());
        assert!(actual.enriched.is_empty());
        assert_eq!(1234, actual.unprocessed.len());
    }

    #[tokio::test]
    async fn should_return_failed_products_for_unknown_shops() {
        let mut repository = MockShopDynamoDbRepository::new();
        let fx_rate = FixedFxRate();

        repository.expect_get_shop_records().returning(|_| {
            Box::pin(async {
                Ok(BatchGetItemResult {
                    items: vec![],
                    unprocessed: None,
                })
            })
        });

        let service = ProductCommandEnrichmentServiceImpl::new(&repository, &fx_rate);
        let cmds = fake::vec![PipedProductCommand; 1234];
        let actual = service.enrich_shop(cmds.clone()).await;

        assert!(actual.unprocessed.is_empty());
        assert!(actual.enriched.is_empty());
        assert_eq!(
            cmds.into_iter()
                .filter(|piped_cmd| {
                    piped_cmd.shop_id.is_none()
                        || piped_cmd.shop_name.is_none()
                        || piped_cmd.shop_type.is_none()
                })
                .count(),
            actual.failed.len()
        );
    }
}

use crate::service::product_command::PipedProductCommand;
use common::{
    batch::Batch,
    price::domain::{FxRate, MonetaryAmountOverflowError},
    shop_id::ShopIdentifier,
};
use shop::core::shop::Shop;
use shop::dynamodb::repository::ShopDynamoDbRepository;
use std::collections::{HashMap, HashSet};
use tracing::error;
use url::Url;

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

    #[error("No Shop with Url '{0}' exists. Cannot enrich shop-information.")]
    UnknownShopUrl(Url),
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
            .map(|cmd| normalize_url(cmd.url.clone()))
            .map(ShopIdentifier::from)
            .collect::<HashSet<_>>();
        let mut shops: HashMap<Url, Shop> = HashMap::with_capacity(shop_identifiers.len());
        let mut unprocessed_shops = HashSet::new();

        for batch in Batch::chunked_from(shop_identifiers.into_iter()) {
            match self.shop_dynamodb_repository.get_shop_records(&batch).await {
                Ok(res) => {
                    if let Some(unprocessed) = res.unprocessed {
                        unprocessed_shops.extend(&mut unprocessed.into_iter());
                    }
                    for record in res.items {
                        for mut url in record.urls.clone() {
                            as_normalized_url(&mut url);
                            shops.insert(url, record.clone().into());
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
            let shop_url = normalize_url(cmd.url.clone());
            let shop_identifier = ShopIdentifier::ShopUrl(shop_url);

            if unprocessed_shops.contains(&shop_identifier) {
                output.unprocessed.push(cmd);
            } else if cmd.shop_id.is_none() || cmd.shop_name.is_none() {
                let shop_url = normalize_url(cmd.url.clone());
                match shops.get(&shop_url) {
                    Some(shop) => {
                        cmd.shop_id = Some(shop.shop_id);
                        cmd.shop_name = Some(shop.name.clone());
                        output.enriched.push(cmd);
                    }
                    None => output
                        .failed
                        .push((cmd, EnrichProductCommandError::UnknownShopUrl(shop_url))),
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
            match other_price_res {
                Ok(other_price) => {
                    cmd.other_price = other_price;
                    output.enriched.push(cmd);
                }
                Err(err) => {
                    output
                        .failed
                        .push((cmd, EnrichProductCommandError::from(err)));
                }
            }
        }

        output
    }
}

fn normalize_url(url: Url) -> Url {
    let mut url = url;
    as_normalized_url(&mut url);
    url
}

fn as_normalized_url(url: &mut Url) {
    url.set_query(None);
    url.set_path("");
}

#[cfg(feature = "test-data")]
mod faker {
    use super::*;
    use common::price::domain::{FixedFxRate, Price};
    use fake::{Dummy, Fake, Faker, Rng};

    impl Dummy<Faker> for PipedProductCommand {
        fn dummy_with_rng<R: Rng + ?Sized>(config: &Faker, rng: &mut R) -> Self {
            let native_price: Option<Price> = config.fake_with_rng(rng);
            let other_price = match native_price {
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
                native_title: config.fake_with_rng(rng),
                other_title: config.fake_with_rng(rng),
                native_description: config.fake_with_rng(rng),
                other_description: config.fake_with_rng(rng),
                native_price,
                other_price,
                state,
                url: Url::parse(&format!(
                    "https://foo.bar/item/{}",
                    config.fake_with_rng::<u16, _>(rng)
                ))
                .unwrap(),
                images: vec![
                    Url::parse(&format!(
                        "https://foo.bar/images/{}",
                        config.fake_with_rng::<u16, _>(rng)
                    ))
                    .unwrap(),
                    Url::parse(&format!(
                        "https://foo.bar/images/{}",
                        config.fake_with_rng::<u16, _>(rng)
                    ))
                    .unwrap(),
                    Url::parse(&format!(
                        "https://foo.bar/images/{}",
                        config.fake_with_rng::<u16, _>(rng)
                    ))
                    .unwrap(),
                ],
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::service::enrichment_service::{
        PipedProductCommand, ProductCommandEnrichmentService, ProductCommandEnrichmentServiceImpl,
        normalize_url,
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
    use url::Url;

    #[rstest::rstest]
    #[case("https://google.com", "https://google.com/")]
    #[case("https://google.com/foo", "https://google.com/")]
    #[case("https://google.com/foo/bar", "https://google.com/")]
    #[case("https://google.com?baz=bat", "https://google.com/")]
    #[case("https://google.com?baz=bat&olga=rego", "https://google.com/")]
    #[case("https://google.com/wau/?baz=bat&olga=rego", "https://google.com/")]
    #[case("https://google.com/wau/miau/?baz=bat&olg=reg", "https://google.com/")]
    fn should_normalize_url(#[case] url: &str, #[case] expected: &str) {
        let url = Url::parse(url).unwrap();

        let actual = normalize_url(url);

        assert_eq!(expected, actual.as_str());
    }

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
                                ShopIdentifier::ShopUrl(url) => url,
                            };
                            shop.urls.push(url);
                            ShopRecord::try_clone_from_shop_as_shop_url_records(&shop).unwrap()
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
                    piped_cmd.shop_id.is_none() || piped_cmd.shop_name.is_none()
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
                    piped_cmd.shop_id.is_none() || piped_cmd.shop_name.is_none()
                })
                .count(),
            actual.failed.len()
        );
    }
}

use common::{
    batch::Batch,
    currency::domain::Currency,
    item_state::domain::ItemState,
    language::domain::Language,
    localized::Localized,
    price::domain::{FxRate, MonetaryAmount, MonetaryAmountOverflowError, Price},
    shop_id::{ShopId, ShopIdentifier},
    shop_name::ShopName,
    shops_item_id::ShopsItemId,
};
use item_core::{description::Description, title::Title};
use shop_core::shop::Shop;
use shop_dynamodb::repository::ShopDynamoDbRepository;
use std::collections::{HashMap, HashSet};
use tracing::error;
use url::Url;

#[derive(Debug, Clone, PartialEq)]
pub struct PipedItemCommand {
    pub shop_id: Option<ShopId>,
    pub shops_item_id: ShopsItemId,
    pub shop_name: Option<ShopName>,
    pub native_title: Localized<Language, Title>,
    pub other_title: HashMap<Language, Title>,
    pub native_description: Option<Localized<Language, Description>>,
    pub other_description: HashMap<Language, Description>,
    pub native_price: Option<Price>,
    pub other_price: HashMap<Currency, MonetaryAmount>,
    pub state: ItemState,
    pub url: Url,
    pub images: Vec<Url>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct EnrichItemCommandsOutput {
    pub enriched: Vec<PipedItemCommand>,
    pub failed: Vec<(PipedItemCommand, EnrichItemCommandError)>,
    pub unprocessed: Vec<PipedItemCommand>,
}

#[derive(thiserror::Error, Debug, Clone, PartialEq, Eq)]
pub enum EnrichItemCommandError {
    #[error("MonetaryAmountOverflowError: {0}")]
    MonetaryAmountOverflowError(#[from] MonetaryAmountOverflowError),

    #[error("No Shop with Url '{0}' exists. Cannot enrich shop-information.")]
    UnknownShopUrl(Url),
}

#[async_trait::async_trait]
#[mockall::automock]
pub trait ItemCommandEnrichmentService {
    async fn enrich_shop(&self, commands: Vec<PipedItemCommand>) -> EnrichItemCommandsOutput;

    fn enrich_price(&self, commands: Vec<PipedItemCommand>) -> EnrichItemCommandsOutput;
}

pub struct ItemCommandEnrichmentServiceImpl<'a, T: FxRate + Sync> {
    shop_dynamodb_repository: &'a (dyn ShopDynamoDbRepository + Sync),
    fx_rate: &'a T,
}

impl<'a, T: FxRate + Sync> ItemCommandEnrichmentServiceImpl<'a, T> {
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
impl<'a, T: FxRate + Sync> ItemCommandEnrichmentService
    for ItemCommandEnrichmentServiceImpl<'a, T>
{
    async fn enrich_shop(&self, commands: Vec<PipedItemCommand>) -> EnrichItemCommandsOutput {
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

        let mut output = EnrichItemCommandsOutput::default();
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
                        .push((cmd, EnrichItemCommandError::UnknownShopUrl(shop_url))),
                }
            }
        }

        output
    }

    fn enrich_price(&self, commands: Vec<PipedItemCommand>) -> EnrichItemCommandsOutput {
        let mut output = EnrichItemCommandsOutput::default();

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
                    output.failed.push((cmd, EnrichItemCommandError::from(err)));
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
    use common::price::domain::FixedFxRate;
    use fake::{Dummy, Fake, Faker, Rng};

    impl Dummy<Faker> for PipedItemCommand {
        fn dummy_with_rng<R: Rng + ?Sized>(config: &Faker, rng: &mut R) -> Self {
            let native_price: Option<Price> = config.fake_with_rng(rng);
            let other_price = match native_price {
                None => HashMap::new(),
                Some(price) => FixedFxRate()
                    .exchange_all(price.currency, price.monetary_amount)
                    .unwrap(),
            };
            let state = config.fake_with_rng(rng);
            PipedItemCommand {
                shop_id: config.fake_with_rng(rng),
                shops_item_id: config.fake_with_rng(rng),
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
    use std::panic;

    use crate::enrichment_service::{
        ItemCommandEnrichmentService, ItemCommandEnrichmentServiceImpl, PipedItemCommand,
        normalize_url,
    };
    use aws_sdk_sqs::error::SdkError;
    use common::{
        batch::dynamodb::BatchGetItemResult, currency::domain::Currency,
        price::domain::FixedFxRate, shop_id::ShopIdentifier,
    };
    use fake::{Fake, Faker};
    use shop_core::shop::Shop;
    use shop_dynamodb::{repository::MockShopDynamoDbRepository, shop_record::ShopRecord};
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
        let service = ItemCommandEnrichmentServiceImpl::new(&repository, &fx_rate);

        let mut cmd = Faker.fake::<PipedItemCommand>();
        cmd.native_price = Some(Faker.fake());
        cmd.other_price.clear();

        let actual = service.enrich_price(vec![cmd]);

        assert_eq!(1, actual.enriched.len());
        assert_eq!(Currency::COUNT, actual.enriched[0].other_price.len());
    }

    #[tokio::test]
    async fn should_return_enriched_items() {
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

        let service = ItemCommandEnrichmentServiceImpl::new(&repository, &fx_rate);
        let cmds = fake::vec![PipedItemCommand; 1234];
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
    async fn should_return_unprocessed_items_for_unprocessed_shops() {
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

        let service = ItemCommandEnrichmentServiceImpl::new(&repository, &fx_rate);
        let actual = service
            .enrich_shop(fake::vec![PipedItemCommand; 1234])
            .await;

        assert!(actual.failed.is_empty());
        assert!(actual.enriched.is_empty());
        assert_eq!(1234, actual.unprocessed.len());
    }

    #[tokio::test]
    async fn should_return_unprocessed_items_for_failed_shops() {
        let mut repository = MockShopDynamoDbRepository::new();
        let fx_rate = FixedFxRate();

        repository.expect_get_shop_records().returning(|_| {
            Box::pin(async { Err(SdkError::construction_failure("Something went wrong")) })
        });

        let service = ItemCommandEnrichmentServiceImpl::new(&repository, &fx_rate);
        let actual = service
            .enrich_shop(fake::vec![PipedItemCommand; 1234])
            .await;

        assert!(actual.failed.is_empty());
        assert!(actual.enriched.is_empty());
        assert_eq!(1234, actual.unprocessed.len());
    }

    #[tokio::test]
    async fn should_return_failed_items_for_unknown_shops() {
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

        let service = ItemCommandEnrichmentServiceImpl::new(&repository, &fx_rate);
        let cmds = fake::vec![PipedItemCommand; 1234];
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

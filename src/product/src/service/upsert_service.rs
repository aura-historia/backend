use crate::core::product::Product;
use crate::core::product_event::ProductEvent;
use crate::dynamodb::product_event_record::ProductEventRecord;
use crate::dynamodb::repository::{ProductDynamoDbRepository, extract_product_key};
use crate::service::product_command::UpsertProductCommand;
use async_trait::async_trait;
use common::batch::Batch;
use common::has_key::HasKey;
use common::price::domain::FxRate;
use common::product_id::ProductKey;
use itertools::Itertools;
use std::collections::HashMap;
use tracing::error;

#[derive(Debug, Clone, PartialEq)]
pub struct UpsertProductsOutput {
    pub unprocessed: Vec<UpsertProductCommand>,
    pub skipped: usize,
}

#[async_trait]
#[mockall::automock]
pub trait UpsertProductsService {
    async fn upsert(&self, commands: Vec<UpsertProductCommand>) -> UpsertProductsOutput;
}

pub struct UpsertProductsServiceImpl<'a, T: FxRate + Sync> {
    dynamodb_repository: &'a (dyn ProductDynamoDbRepository + Sync),
    fx_rate: &'a T,
}

impl<'a, T: FxRate + Sync> UpsertProductsServiceImpl<'a, T> {
    pub fn new(
        dynamodb_repository: &'a (dyn ProductDynamoDbRepository + Sync),
        fx_rate: &'a T,
    ) -> Self {
        Self {
            dynamodb_repository,
            fx_rate,
        }
    }
}

#[async_trait]
impl<T: FxRate + Sync> UpsertProductsService for UpsertProductsServiceImpl<'_, T> {
    async fn upsert(&self, commands: Vec<UpsertProductCommand>) -> UpsertProductsOutput {
        let chunks = commands
            .into_iter()
            .chunks(100)
            .into_iter()
            .map(|chunk| chunk.collect::<Vec<_>>())
            .collect::<Vec<_>>();

        let mut skipped = 0;
        let mut unprocessed = Vec::new();
        for chunk in chunks {
            let batch: Batch<UpsertProductCommand, 100> = chunk
                .try_into()
                .expect("shouldn't fail converting chunk of size 100 to Batch of size 100");
            let mut failed = self.handle_put_chunk_with_retry(batch, &mut skipped).await;
            unprocessed.append(&mut failed);
        }

        UpsertProductsOutput {
            unprocessed,
            skipped,
        }
    }
}

impl<T: FxRate + Sync> UpsertProductsServiceImpl<'_, T> {
    async fn handle_put_chunk_with_retry(
        &self,
        chunk: Batch<UpsertProductCommand, 100>,
        skipped_count: &mut usize,
    ) -> Vec<UpsertProductCommand> {
        const MAX_RETRIES: u32 = 5;
        const BASE_DELAY_MS: u64 = 100;

        let mut current_chunk = chunk;
        let mut retry_count = 0;
        loop {
            let failed = self.handle_put_chunk(current_chunk, skipped_count).await;
            if failed.is_empty() || retry_count >= MAX_RETRIES {
                return failed;
            }

            retry_count += 1;
            let delay_ms = BASE_DELAY_MS * 2_u64.pow(retry_count - 1);
            tokio::time::sleep(tokio::time::Duration::from_millis(delay_ms)).await;

            current_chunk = failed
                .try_into()
                .expect("shouldn't fail converting failed products back to Batch because they came from a valid Batch");
        }
    }

    async fn handle_put_chunk(
        &self,
        chunk: Batch<UpsertProductCommand, 100>,
        skipped_count: &mut usize,
    ) -> Vec<UpsertProductCommand> {
        let mut key_cmds = chunk
            .into_iter()
            .map(|cmd| (cmd.key(), cmd))
            .collect::<HashMap<_, _>>();
        let mut mut_key_cmds = key_cmds.clone();
        let keys = mut_key_cmds
            .keys()
            .cloned()
            .collect::<Vec<_>>()
            .try_into()
            .expect("shoulnd't fail unwrapping created 'Batch' because 'iter()' keeps size");

        match self.dynamodb_repository.get_product_records(&keys).await {
            Ok(records) => {
                let mut unprocessed_failures = Vec::with_capacity(
                    records
                        .unprocessed
                        .as_ref()
                        .map(|batch| batch.len())
                        .unwrap_or(0),
                );
                if let Some(unprocessed) = records.unprocessed {
                    for unprocessed_key in unprocessed {
                        match mut_key_cmds.remove(&unprocessed_key) {
                            Some(update_cmd) => unprocessed_failures.push(update_cmd),
                            None => {
                                error!(
                                    shopId = %unprocessed_key.shop_id,
                                    shopsProductId = %unprocessed_key.shops_product_id,
                                    "Couldn't find PutItemCommand for unprocessed Product. This is a bug. Not retrying."
                                );
                            }
                        }
                    }
                }

                let mut update_chunk = Vec::with_capacity(records.items.len());
                for record in records.items {
                    match mut_key_cmds.remove(&record.key()) {
                        Some(update_cmd) => update_chunk.push((Product::from(record), update_cmd)),
                        None => {
                            error!(
                                shopId = %record.shop_id,
                                shopsProductId = %record.shops_product_id,
                                "Couldn't find PutItemCommand for Product proven to exist. This is a bug. Not retrying."
                            );
                        }
                    }
                }
                let update_events = self
                    .extract_update_events(update_chunk, skipped_count)
                    .await;

                // all remaining commands must be for products that don't yet exist - so we create them now
                let mut create_events = self
                    .extract_create_events(mut_key_cmds.into_values().collect())
                    .await;

                let mut events = update_events;
                events.append(&mut create_events);

                let batches = Batch::<_, 25>::chunked_from(events.into_iter());
                for batch in batches {
                    let product_keys = batch
                        .iter()
                        .map(|event| ProductKey {
                            shop_id: event.shop_id,
                            shops_product_id: event.shops_product_id.clone(),
                        })
                        .collect::<Vec<_>>();
                    let res = self
                        .dynamodb_repository
                        .put_product_event_records(batch)
                        .await;
                    match res {
                        Ok(output) => {
                            let failed_product_keys = output
                                .unprocessed_items
                                .unwrap_or_default()
                                .into_iter()
                                .flat_map(|(_table, reqs)| reqs)
                                .map(|req| req.put_request.expect("shouldn't be any other request than 'PutRequest' because events are append-only").item)
                                .map(extract_product_key)
                                .filter_map(|result| match result {
                                    Ok(event) => Some(event),
                                    Err(err) => {
                                        error!(error = %err, "Failed extracting ProductKey.");
                                        None
                                    }
                                });
                            for failed_product_key in failed_product_keys {
                                match key_cmds.remove(&failed_product_key) {
                                    Some(cmd) => unprocessed_failures.push(cmd.clone()),
                                    None => {
                                        let already_failed_command = unprocessed_failures
                                            .iter()
                                            .any(|unprocessed_failure| {
                                                unprocessed_failure.shop_id
                                                    == failed_product_key.shop_id
                                                    && unprocessed_failure.shops_product_id
                                                        == failed_product_key.shops_product_id
                                            });
                                        if !already_failed_command {
                                            error!(
                                                shopId = %failed_product_key.shop_id,
                                                shopsProductId = %failed_product_key.shops_product_id,
                                                "Couldn't find PutItemCommand for unprocessed message. This is a bug. Not retrying."
                                            );
                                        }
                                    }
                                }
                            }
                        }
                        Err(err) => {
                            error!(error = ?err, "Failed writing entire ProductEventRecord-Batch due to SdkError.");
                            for product_key in product_keys {
                                match key_cmds.remove(&product_key) {
                                    Some(cmd) => unprocessed_failures.push(cmd),
                                    None => {
                                        error!(
                                            shopId = %product_key.shop_id,
                                            shopsProductId = %product_key.shops_product_id,
                                            "Couldn't find PutItemCommand for unprocessed message. This is a bug. Not retrying."
                                        );
                                    }
                                }
                            }
                        }
                    }
                }

                unprocessed_failures
            }
            Err(err) => {
                error!(err = ?err, "Failed entire BatchGetItem-Operation.");
                mut_key_cmds.into_values().collect()
            }
        }
    }

    async fn extract_create_events(
        &self,
        create_chunk: Vec<UpsertProductCommand>,
    ) -> Vec<ProductEventRecord> {
        create_chunk.into_iter().map(|cmd| {
            let other_price = cmd
                .native_price
                .as_ref()
                .and_then(|price| {
                    self.fx_rate
                        .exchange_all(price.currency, price.monetary_amount)
                        .map(Some)
                        .unwrap_or_else(|err| {
                            error!(error = %err, price = ?price, "Failed exchanging price for all other supported currencies.");
                            None
                        })
                })
                .unwrap_or_default();
            let other_price_estimate_min = cmd
                .native_price_estimate_min
                .as_ref()
                .and_then(|price| {
                    self.fx_rate
                        .exchange_all(price.currency, price.monetary_amount)
                        .map(Some)
                        .unwrap_or_else(|err| {
                            error!(error = %err, price = ?price, "Failed exchanging price estimate min for all other supported currencies.");
                            None
                        })
                })
                .unwrap_or_default();
            let other_price_estimate_max = cmd
                .native_price_estimate_max
                .as_ref()
                .and_then(|price| {
                    self.fx_rate
                        .exchange_all(price.currency, price.monetary_amount)
                        .map(Some)
                        .unwrap_or_else(|err| {
                            error!(error = %err, price = ?price, "Failed exchanging price estimate max for all other supported currencies.");
                            None
                        })
                })
                .unwrap_or_default();
            Product::create(
                cmd.shop_id,
                cmd.shops_product_id,
                cmd.shop_name,
                cmd.shop_type,
                cmd.native_title,
                cmd.native_description,
                cmd.native_price,
                other_price,
                cmd.native_price_estimate_min,
                other_price_estimate_min,
                cmd.native_price_estimate_max,
                other_price_estimate_max,
                cmd.state,
                cmd.url,
                cmd.images,
                cmd.auction_start,
                cmd.auction_end,
            )
        })
        .filter_map(|event| {
            match ProductEventRecord::try_from(event) {
                Ok(record_event) => Some(record_event),
                Err(err) => {
                    error!(error = %err, "Failed converting ProductEvent to ProductEventRecord. This is a bug. Not retrying");
                    None
                }
            }
        })
        .collect()
    }

    async fn extract_update_events(
        &self,
        update_chunk: Vec<(Product, UpsertProductCommand)>,
        skipped_count: &mut usize,
    ) -> Vec<ProductEventRecord> {
        determine_update_events(update_chunk, skipped_count, self.fx_rate)
            .into_iter()
            .filter_map(|event| {
                match ProductEventRecord::try_from(event) {
                    Ok(record_event) => Some(record_event),
                    Err(err) => {
                        error!(error = %err, "Failed converting ProductEvent to ProductEventRecord. This is a bug. Not retriying.");
                        None
                    }
                }
            })
        .collect()
    }
}

fn determine_update_events(
    update_chunk: Vec<(Product, UpsertProductCommand)>,
    skipped_count: &mut usize,
    fx_rate: &impl FxRate,
) -> Vec<ProductEvent> {
    let mut events = Vec::with_capacity(update_chunk.len());
    for (mut product, update_cmd) in update_chunk {
        let mut any_changes = false;

        if let Some(price_event) = product.new_price(update_cmd.native_price, fx_rate) {
            events.push(price_event);
            any_changes = true;
        }
        if let Some(state_event) = product.change_state(update_cmd.state) {
            events.push(state_event);
            any_changes = true;
        }

        if !any_changes {
            *skipped_count += 1;
        }
    }

    events
}

#[cfg(test)]
pub mod tests {
    use crate::core::product::Product;
    use crate::dynamodb::repository::MockProductDynamoDbRepository;
    use crate::service::product_command::UpsertProductCommand;
    use crate::service::upsert_service::{UpsertProductsServiceImpl, determine_update_events};
    use aws_sdk_dynamodb::error::SdkError;
    use common::has_key::HasKey;
    use common::{price::domain::FixedFxRate, product_state::domain::ProductState};
    use fake::{Fake, Faker};
    use rstest;

    #[test]
    fn should_determine_no_update_events_when_only_skipped() {
        let product1 = Faker.fake::<Product>();
        let mut update1 = Faker.fake::<UpsertProductCommand>();
        update1.native_price = product1.native_price;
        update1.state = product1.state;

        let product2 = Faker.fake::<Product>();
        let mut update2 = Faker.fake::<UpsertProductCommand>();
        update2.native_price = product2.native_price;
        update2.state = product2.state;

        let mut skipped_count = 0;
        let update_chunk = vec![(product1, update1), (product2, update2)];

        let actual = determine_update_events(update_chunk, &mut skipped_count, &FixedFxRate());
        assert_eq!(2, skipped_count);
        assert!(actual.is_empty());
    }

    #[test]
    fn should_determine_update_events_when_none_skipped() {
        let product1 = Faker.fake::<Product>();
        let update1 = UpsertProductCommand {
            shop_id: product1.clone().shop_id,
            shops_product_id: product1.clone().shops_product_id,
            shop_name: product1.clone().shop_name,
            shop_type: Faker.fake(),
            native_title: product1.clone().native_title,
            other_title: Default::default(),
            native_description: product1.clone().native_description,
            other_description: Default::default(),
            native_price: Some(Faker.fake()),
            other_price: Default::default(),
            state: product1.state,
            url: product1.clone().url,
            images: product1.clone().images,
            auction_start: product1.auction_start,
            auction_end: product1.auction_end,
        };

        let product2 = Faker.fake::<Product>();
        let update2 = UpsertProductCommand {
            shop_id: product2.clone().shop_id,
            shops_product_id: product2.clone().shops_product_id,
            shop_name: product2.clone().shop_name,
            native_title: product2.clone().native_title,
            other_title: Default::default(),
            shop_type: product2.clone().shop_type,
            native_description: product2.clone().native_description,
            other_description: Default::default(),
            native_price: Some(Faker.fake()),
            other_price: Default::default(),
            state: if matches!(product2.state, ProductState::Available) {
                ProductState::Removed
            } else {
                ProductState::Available
            },
            url: product2.clone().url,
            images: product2.clone().images,
            auction_start: product2.auction_start,
            auction_end: product2.auction_end,
        };

        let mut skipped_count = 0;
        let update_chunk = vec![(product1, update1), (product2, update2)];

        let actual = determine_update_events(update_chunk, &mut skipped_count, &FixedFxRate());
        assert_eq!(0, skipped_count);
        assert_eq!(3, actual.len())
    }

    #[test]
    fn should_determine_update_events_when_some_skipped() {
        let product1 = Faker.fake::<Product>();
        let update1 = UpsertProductCommand {
            shop_id: product1.clone().shop_id,
            shops_product_id: product1.clone().shops_product_id,
            shop_name: product1.clone().shop_name,
            shop_type: Faker.fake(),
            native_title: product1.clone().native_title,
            other_title: Default::default(),
            native_description: product1.clone().native_description,
            other_description: Default::default(),
            native_price: Some(Faker.fake()),
            other_price: Default::default(),
            state: product1.state,
            url: product1.clone().url,
            images: product1.clone().images,
            auction_start: product1.auction_start,
            auction_end: product1.auction_end,
        };

        let product2 = Faker.fake::<Product>();
        let mut update2 = Faker.fake::<UpsertProductCommand>();
        update2.native_price = product2.native_price;
        update2.state = product2.state;

        let mut skipped_count = 0;
        let update_chunk = vec![(product1, update1), (product2, update2)];

        let actual = determine_update_events(update_chunk, &mut skipped_count, &FixedFxRate());
        assert_eq!(1, skipped_count);
        assert_eq!(1, actual.len())
    }

    #[tokio::test]
    #[rstest::rstest]
    #[case::construction_failure(SdkError::construction_failure("Something went wrong"))]
    #[case::timeout(SdkError::timeout_error("Something went wrong"))]
    #[case::dispatch_failure(SdkError::dispatch_failure(aws_sdk_dynamodb::error::ConnectorError::user("Something went wrong".into())))]
    #[case::response_error(SdkError::response_error(
        "Something went wrong",
        aws_sdk_dynamodb::config::http::HttpResponse::new(500u16.try_into().unwrap(), "{}".into())
    ))]
    #[case::service_error(SdkError::service_error(
        aws_sdk_dynamodb::operation::batch_get_item::BatchGetItemError::unhandled("Something went wrong"),
        aws_sdk_dynamodb::config::http::HttpResponse::new(500u16.try_into().unwrap(), "{}".into())
    ))]
    #[trace]
    async fn should_fail_entire_chunk_when_batch_get_product_entirely_fails(
        #[case] expected: SdkError<
            aws_sdk_dynamodb::operation::batch_get_item::BatchGetItemError,
            aws_sdk_dynamodb::config::http::HttpResponse,
        >,
    ) {
        let mut repository = MockProductDynamoDbRepository::default();
        repository
            .expect_get_product_records()
            .return_once(|_| Box::pin(async { Err(expected) }));

        let service = UpsertProductsServiceImpl::new(&repository, &FixedFxRate());

        let mut skipped_count = 0;
        let mut expected = fake::vec![UpsertProductCommand; 89];
        let mut actual = service
            .handle_put_chunk(expected.clone().try_into().unwrap(), &mut skipped_count)
            .await;

        expected.sort_by_key(|l| l.key());
        actual.sort_by_key(|l| l.key());

        assert_eq!(expected, actual);
    }

    #[tokio::test]
    async fn should_apply_exponential_backoff_when_retrying() {
        use common::batch::dynamodb::BatchGetItemResult;
        use std::time::Instant;

        let mut repository = MockProductDynamoDbRepository::default();

        let commands = fake::vec![UpsertProductCommand; 3];

        // All calls fail (initial + 5 retries = 6 total)
        repository
            .expect_get_product_records()
            .times(6)
            .returning(|keys| {
                let unprocessed_keys: Vec<_> = keys.iter().cloned().collect();
                Box::pin(async move {
                    Ok(BatchGetItemResult {
                        items: vec![],
                        unprocessed: Some(unprocessed_keys.try_into().unwrap()),
                    })
                })
            });

        let service = UpsertProductsServiceImpl::new(&repository, &FixedFxRate());

        let mut skipped_count = 0;

        let start = Instant::now();
        let _ = service
            .handle_put_chunk_with_retry(commands.try_into().unwrap(), &mut skipped_count)
            .await;
        let elapsed = start.elapsed();

        // Expected delays: 100ms + 200ms + 400ms + 800ms + 1600ms = 3100ms minimum
        // Allow some tolerance for execution time
        assert!(
            elapsed.as_millis() >= 3100,
            "Expected at least 3100ms for exponential backoff, got {}ms",
            elapsed.as_millis()
        );
        assert!(
            elapsed.as_millis() < 10000,
            "Expected less than 10000ms total, got {}ms",
            elapsed.as_millis()
        );
    }
}

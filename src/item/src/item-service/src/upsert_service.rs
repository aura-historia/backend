use crate::item_command::UpsertItemCommand;
use async_trait::async_trait;
use common::batch::Batch;
use common::has_key::HasKey;
use common::price::domain::FxRate;
use item_core::item::Item;
use item_core::item_event::ItemEvent;
use item_dynamodb::item_event_record::ItemEventRecord;
use item_dynamodb::repository::ItemDynamoDbRepository;
use itertools::Itertools;
use std::collections::HashMap;
use tracing::error;

#[derive(Debug, Clone, PartialEq)]
pub struct UpsertItemsOutput {
    pub unprocessed: Vec<UpsertItemCommand>,
    pub skipped: usize,
}

#[async_trait]
#[mockall::automock]
pub trait UpsertItemsService {
    async fn upsert(&self, commands: Vec<UpsertItemCommand>) -> UpsertItemsOutput;
}

pub struct UpsertItemsServiceImpl<'a, T: FxRate + Sync> {
    dynamodb_repository: &'a (dyn ItemDynamoDbRepository + Sync),
    sqs_client: &'a aws_sdk_sqs::Client,
    item_ingest_events_dynamodb_queue_url: &'a str,
    fx_rate: &'a T,
}

impl<'a, T: FxRate + Sync> UpsertItemsServiceImpl<'a, T> {
    pub fn new(
        dynamodb_repository: &'a (dyn ItemDynamoDbRepository + Sync),
        sqs_client: &'a aws_sdk_sqs::Client,
        item_ingest_events_dynamodb_queue: &'a str,
        fx_rate: &'a T,
    ) -> Self {
        Self {
            dynamodb_repository,
            sqs_client,
            item_ingest_events_dynamodb_queue_url: item_ingest_events_dynamodb_queue,
            fx_rate,
        }
    }
}

#[async_trait]
impl<T: FxRate + Sync> UpsertItemsService for UpsertItemsServiceImpl<'_, T> {
    async fn upsert(&self, commands: Vec<UpsertItemCommand>) -> UpsertItemsOutput {
        let chunks = commands
            .into_iter()
            .chunks(100)
            .into_iter()
            .map(|chunk| chunk.collect::<Vec<_>>())
            .collect::<Vec<_>>();

        let mut skipped = 0;
        let mut unprocessed = Vec::new();
        for chunk in chunks {
            let batch: Batch<UpsertItemCommand, 100> = chunk
                .try_into()
                .expect("shouldn't fail converting chunk of size 100 to Batch of size 100");
            let mut failed = self.handle_put_chunk_with_retry(batch, &mut skipped).await;
            unprocessed.append(&mut failed);
        }

        UpsertItemsOutput {
            unprocessed,
            skipped,
        }
    }
}

impl<T: FxRate + Sync> UpsertItemsServiceImpl<'_, T> {
    async fn handle_put_chunk_with_retry(
        &self,
        chunk: Batch<UpsertItemCommand, 100>,
        skipped_count: &mut usize,
    ) -> Vec<UpsertItemCommand> {
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
                .expect("shouldn't fail converting failed items back to Batch because they came from a valid Batch");
        }
    }

    async fn handle_put_chunk(
        &self,
        chunk: Batch<UpsertItemCommand, 100>,
        skipped_count: &mut usize,
    ) -> Vec<UpsertItemCommand> {
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

        match self.dynamodb_repository.get_item_records(&keys).await {
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
                                    shopsItemId = %unprocessed_key.shops_item_id,
                                    "Couldn't find PutItemCommand for unprocessed Item. This is a bug. Not retrying."
                                );
                            }
                        }
                    }
                }

                let mut update_chunk = Vec::with_capacity(records.items.len());
                for record in records.items {
                    match mut_key_cmds.remove(&record.key()) {
                        Some(update_cmd) => update_chunk.push((Item::from(record), update_cmd)),
                        None => {
                            error!(
                                shopId = %record.shop_id,
                                shopsItemId = %record.shops_item_id,
                                "Couldn't find PutItemCommand for Item proven to exist. This is a bug. Not retrying."
                            );
                        }
                    }
                }
                let update_events = self
                    .extract_update_events(update_chunk, skipped_count)
                    .await;

                // all remaining commands must be for items that don't yet exist - so we create them now
                let mut create_events = self
                    .extract_create_events(mut_key_cmds.into_values().collect())
                    .await;

                let mut events = update_events;
                events.append(&mut create_events);

                let batches = Batch::<_, 10>::chunked_from(events.into_iter());
                for batch in batches {
                    let msg_key = batch
                        .iter()
                        .enumerate()
                        .map(|(i, record)| (i.to_string(), record.key()))
                        .collect::<HashMap<_, _>>();
                    let res = self
                        .sqs_client
                        .send_message_batch()
                        .queue_url(self.item_ingest_events_dynamodb_queue_url)
                        .set_entries(Some(batch.into_sqs_message_entries()))
                        .send()
                        .await;
                    match res {
                        Ok(output) => {
                            for failed in output.failed {
                                match msg_key.get(failed.id()) {
                                    Some(failed_key) => match key_cmds.remove(failed_key) {
                                        Some(cmd) => unprocessed_failures.push(cmd),
                                        None => {
                                            error!(
                                                shopId = %failed_key.shop_id,
                                                shopsItemId = %failed_key.shops_item_id,
                                                "Couldn't find PutItemCommand for unproccesed message. This is a bug. Not retrying."
                                            );
                                        }
                                    },
                                    None => error!(
                                        payload = ?failed,
                                        "Couldn't find ItemKey for unproccesed message. This is a bug. Not retrying."
                                    ),
                                }
                            }
                        }
                        Err(err) => {
                            error!(error = ?err, "Failed writing entire ItemEventRecord-Batch due to SdkError.");
                            for key in msg_key.into_values() {
                                match key_cmds.remove(&key) {
                                    Some(cmd) => unprocessed_failures.push(cmd),
                                    None => {
                                        error!(
                                            shopId = %key.shop_id,
                                            shopsItemId = %key.shops_item_id,
                                            "Couldn't find PutItemCommand for unproccesed message. This is a bug. Not retrying."
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
        create_chunk: Vec<UpsertItemCommand>,
    ) -> Vec<ItemEventRecord> {
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
            Item::create(
                cmd.shop_id,
                cmd.shops_item_id,
                cmd.shop_name,
                cmd.native_title,
                Default::default(),
                cmd.native_description,
                Default::default(),
                cmd.native_price,
                other_price,
                cmd.state,
                cmd.url,
                cmd.images,
            )
        })
        .filter_map(|event| {
            match ItemEventRecord::try_from(event) {
                Ok(record_event) => Some(record_event),
                Err(err) => {
                    error!(error = %err, "Failed converting ItemEvent to ItemEventRecord. This is a bug. Not retrying");
                    None
                }
            }
        })
        .collect()
    }

    async fn extract_update_events(
        &self,
        update_chunk: Vec<(Item, UpsertItemCommand)>,
        skipped_count: &mut usize,
    ) -> Vec<ItemEventRecord> {
        determine_update_events(update_chunk, skipped_count, self.fx_rate)
            .into_iter()
            .filter_map(|event| {
                match ItemEventRecord::try_from(event) {
                    Ok(record_event) => Some(record_event),
                    Err(err) => {
                        error!(error = %err, "Failed converting ItemEvent to ItemEventRecord. This is a bug. Not retriying.");
                        None
                    }
                }
            })
        .collect()
    }
}

fn determine_update_events(
    update_chunk: Vec<(Item, UpsertItemCommand)>,
    skipped_count: &mut usize,
    fx_rate: &impl FxRate,
) -> Vec<ItemEvent> {
    let mut events = Vec::with_capacity(update_chunk.len());
    for (mut item, update_cmd) in update_chunk {
        let mut any_changes = false;

        if let Some(price_event) = item.new_price(update_cmd.native_price, fx_rate) {
            events.push(price_event);
            any_changes = true;
        }
        if let Some(state_event) = item.change_state(update_cmd.state) {
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
    use crate::item_command::UpsertItemCommand;
    use crate::upsert_service::{UpsertItemsServiceImpl, determine_update_events};
    use aws_config::BehaviorVersion;
    use aws_sdk_dynamodb::error::SdkError;
    use common::has_key::HasKey;
    use common::{item_state::domain::ItemState, price::domain::FixedFxRate};
    use fake::{Fake, Faker};
    use item_core::item::Item;
    use item_dynamodb::repository::MockItemDynamoDbRepository;

    #[test]
    fn should_determine_no_update_events_when_only_skipped() {
        let item1 = Faker.fake::<Item>();
        let mut update1 = Faker.fake::<UpsertItemCommand>();
        update1.native_price = item1.native_price;
        update1.state = item1.state;

        let item2 = Faker.fake::<Item>();
        let mut update2 = Faker.fake::<UpsertItemCommand>();
        update2.native_price = item2.native_price;
        update2.state = item2.state;

        let mut skipped_count = 0;
        let update_chunk = vec![(item1, update1), (item2, update2)];

        let actual = determine_update_events(update_chunk, &mut skipped_count, &FixedFxRate());
        assert_eq!(2, skipped_count);
        assert!(actual.is_empty());
    }

    #[test]
    fn should_determine_update_events_when_none_skipped() {
        let item1 = Faker.fake::<Item>();
        let update1 = UpsertItemCommand {
            shop_id: item1.clone().shop_id,
            shops_item_id: item1.clone().shops_item_id,
            shop_name: item1.clone().shop_name,
            native_title: item1.clone().native_title,
            other_title: Default::default(),
            native_description: item1.clone().native_description,
            other_description: Default::default(),
            native_price: Some(Faker.fake()),
            other_price: Default::default(),
            state: item1.state,
            url: item1.clone().url,
            images: item1.clone().images,
        };

        let item2 = Faker.fake::<Item>();
        let update2 = UpsertItemCommand {
            shop_id: item2.clone().shop_id,
            shops_item_id: item2.clone().shops_item_id,
            shop_name: item2.clone().shop_name,
            native_title: item2.clone().native_title,
            other_title: Default::default(),
            native_description: item2.clone().native_description,
            other_description: Default::default(),
            native_price: Some(Faker.fake()),
            other_price: Default::default(),
            state: if matches!(item2.state, ItemState::Available) {
                ItemState::Removed
            } else {
                ItemState::Available
            },
            url: item2.clone().url,
            images: item2.clone().images,
        };

        let mut skipped_count = 0;
        let update_chunk = vec![(item1, update1), (item2, update2)];

        let actual = determine_update_events(update_chunk, &mut skipped_count, &FixedFxRate());
        assert_eq!(0, skipped_count);
        assert_eq!(3, actual.len())
    }

    #[test]
    fn should_determine_update_events_when_some_skipped() {
        let item1 = Faker.fake::<Item>();
        let update1 = UpsertItemCommand {
            shop_id: item1.clone().shop_id,
            shops_item_id: item1.clone().shops_item_id,
            shop_name: item1.clone().shop_name,
            native_title: item1.clone().native_title,
            other_title: Default::default(),
            native_description: item1.clone().native_description,
            other_description: Default::default(),
            native_price: Some(Faker.fake()),
            other_price: Default::default(),
            state: item1.state,
            url: item1.clone().url,
            images: item1.clone().images,
        };

        let item2 = Faker.fake::<Item>();
        let mut update2 = Faker.fake::<UpsertItemCommand>();
        update2.native_price = item2.native_price;
        update2.state = item2.state;

        let mut skipped_count = 0;
        let update_chunk = vec![(item1, update1), (item2, update2)];

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
    async fn should_fail_entire_chunk_when_batch_get_item_entirely_fails(
        #[case] expected: SdkError<
            aws_sdk_dynamodb::operation::batch_get_item::BatchGetItemError,
            aws_sdk_dynamodb::config::http::HttpResponse,
        >,
    ) {
        let mut repository = MockItemDynamoDbRepository::default();
        repository
            .expect_get_item_records()
            .return_once(|_| Box::pin(async { Err(expected) }));

        let sqs_client =
            aws_sdk_sqs::Client::new(&aws_config::defaults(BehaviorVersion::latest()).load().await);
        let service =
            UpsertItemsServiceImpl::new(&repository, &sqs_client, "ingest-q-url", &FixedFxRate());

        let mut skipped_count = 0;
        let mut expected = fake::vec![UpsertItemCommand; 89];
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

        let mut repository = MockItemDynamoDbRepository::default();

        let commands = fake::vec![UpsertItemCommand; 3];

        // All calls fail (initial + 5 retries = 6 total)
        repository
            .expect_get_item_records()
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

        let sqs_client =
            aws_sdk_sqs::Client::new(&aws_config::defaults(BehaviorVersion::latest()).load().await);
        let service =
            UpsertItemsServiceImpl::new(&repository, &sqs_client, "ingest-q-url", &FixedFxRate());

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

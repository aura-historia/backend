use crate::core::product::Product;
use crate::dynamodb::product_event_record::ProductEventRecord;
use crate::dynamodb::product_event_record::domain::ProductDomainEventRecord;
use crate::dynamodb::repository::{ProductDynamoDbRepository, extract_product_key};
use crate::service::heuristics;
use crate::service::product_command::{
    CreateProductCommand, UpdateProductCommand, UpsertProductCommand,
};
use async_trait::async_trait;
use common::batch::Batch;
use common::category_key::CategoryId;
use common::has_key::HasKey;
use common::period_key::PeriodId;
use common::price::domain::FxRate;
use common::product_id::ProductKey;
use product_classification::category::service::CategoryService;
use product_classification::period::service::PeriodService;
use std::collections::HashMap;
use tokio::sync::OnceCell;
use tracing::{error, warn};

#[async_trait]
#[mockall::automock]
pub trait CommandProductService {
    async fn create(&self, cmds: Vec<CreateProductCommand>) -> Vec<CreateProductCommand>;
    async fn update(
        &self,
        cmds: HashMap<ProductKey, UpdateProductCommand>,
    ) -> HashMap<ProductKey, UpdateProductCommand>;
    async fn upsert(&self, cmds: Vec<UpsertProductCommand>) -> Vec<UpsertProductCommand>;
}

pub struct CommandProductServiceImpl<'a, T: FxRate + Sync> {
    dynamodb_repository: &'a (dyn ProductDynamoDbRepository + Sync),
    fx_rate: &'a T,
    period_service: &'a (dyn PeriodService + Sync),
    category_service: &'a (dyn CategoryService + Sync),
    classification_cache: OnceCell<ClassificationCache>,
}

struct ClassificationCache {
    period_keywords: Vec<(String, PeriodId)>,
    category_keywords: Vec<(String, CategoryId)>,
}

impl<'a, T: FxRate + Sync> CommandProductServiceImpl<'a, T> {
    pub fn new(
        dynamodb_repository: &'a (dyn ProductDynamoDbRepository + Sync),
        fx_rate: &'a T,
        period_service: &'a (dyn PeriodService + Sync),
        category_service: &'a (dyn CategoryService + Sync),
    ) -> Self {
        Self {
            dynamodb_repository,
            fx_rate,
            period_service,
            category_service,
            classification_cache: OnceCell::new(),
        }
    }

    async fn classification_cache(&self) -> &ClassificationCache {
        self.classification_cache
            .get_or_init(|| async {
                let mut period_keywords: Vec<(String, PeriodId)> =
                    match self.period_service.find_periods().await {
                        Ok(periods) => periods
                            .into_iter()
                            .flat_map(|p| {
                                let id = p.period_id;
                                p.meta_keywords
                                    .into_iter()
                                    .map(move |kw| (kw.as_ref().to_lowercase(), id.clone()))
                            })
                            .collect(),
                        Err(err) => {
                            warn!(error = ?err, "Failed to load periods for classification cache. Period classification will be skipped.");
                            Vec::new()
                        }
                    };
                period_keywords.sort_by(|a, b| b.0.len().cmp(&a.0.len()));

                let mut category_keywords: Vec<(String, CategoryId)> = match self
                    .category_service
                    .find_categories()
                    .await
                {
                    Ok(categories) => categories
                        .into_iter()
                        .flat_map(|c| {
                            let id = c.category_id;
                            c.meta_keywords
                                .into_iter()
                                .map(move |kw| (kw.as_ref().to_lowercase(), id.clone()))
                        })
                        .collect(),
                    Err(err) => {
                        warn!(error = ?err, "Failed to load categories for classification cache. Category classification will be skipped.");
                        Vec::new()
                    }
                };
                category_keywords.sort_by(|a, b| b.0.len().cmp(&a.0.len()));

                ClassificationCache {
                    period_keywords,
                    category_keywords,
                }
            })
            .await
    }

    fn enrich_price(&self, cmd: &mut CreateProductCommand) {
        if let Some(price) = &cmd.native_price {
            match self
                .fx_rate
                .exchange_all(price.currency, price.monetary_amount)
            {
                Ok(other) => cmd.other_price = other,
                Err(err) => {
                    error!(error = %err, "Failed to convert native_price. Defaulting to empty.")
                }
            }
        }
        if let Some(price) = &cmd.native_price_estimate_min {
            match self
                .fx_rate
                .exchange_all(price.currency, price.monetary_amount)
            {
                Ok(other) => cmd.other_price_estimate_min = other,
                Err(err) => {
                    error!(error = %err, "Failed to convert native_price_estimate_min. Defaulting to empty.")
                }
            }
        }
        if let Some(price) = &cmd.native_price_estimate_max {
            match self
                .fx_rate
                .exchange_all(price.currency, price.monetary_amount)
            {
                Ok(other) => cmd.other_price_estimate_max = other,
                Err(err) => {
                    error!(error = %err, "Failed to convert native_price_estimate_max. Defaulting to empty.")
                }
            }
        }
    }

    async fn persist_events<C>(
        &self,
        events: Vec<ProductEventRecord>,
        key_cmds: &mut HashMap<ProductKey, C>,
    ) -> Vec<(ProductKey, C)> {
        let mut failures = Vec::new();
        for batch in Batch::<_, 25>::chunked_from(events.into_iter()) {
            let product_keys = batch.iter().map(|event| event.key()).collect::<Vec<_>>();
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
                            Ok(key) => Some(key),
                            Err(err) => {
                                error!(error = %err, "Failed extracting ProductKey.");
                                None
                            }
                        });
                    for failed_product_key in failed_product_keys {
                        if let Some(cmd) = key_cmds.remove(&failed_product_key) {
                            failures.push((failed_product_key, cmd));
                        }
                    }
                }
                Err(err) => {
                    error!(error = ?err, "Failed writing entire ProductEventRecord-Batch due to SdkError.");
                    for product_key in product_keys {
                        if let Some(cmd) = key_cmds.remove(&product_key) {
                            failures.push((product_key, cmd));
                        }
                    }
                }
            }
        }
        failures
    }
}

#[async_trait]
impl<T: FxRate + Sync> CommandProductService for CommandProductServiceImpl<'_, T> {
    async fn create(&self, cmds: Vec<CreateProductCommand>) -> Vec<CreateProductCommand> {
        let mut failures = Vec::new();
        let cache = self.classification_cache().await;

        for chunk in Batch::<CreateProductCommand, 100>::chunked_from(cmds.into_iter()) {
            let mut key_cmds: HashMap<ProductKey, CreateProductCommand> =
                chunk.into_iter().map(|cmd| (cmd.key(), cmd)).collect();
            let mut working = key_cmds.clone();
            let keys: Batch<ProductKey, 100> = working
                .keys()
                .cloned()
                .collect::<Vec<_>>()
                .try_into()
                .expect("shouldn't fail because keys come from a Batch<_, 100>");

            match self.dynamodb_repository.get_product_records(&keys).await {
                Ok(records) => {
                    if let Some(unprocessed) = records.unprocessed {
                        for key in unprocessed {
                            if let Some(cmd) = working.remove(&key) {
                                failures.push(cmd);
                            }
                        }
                    }

                    for record in records.items {
                        let key = record.key();
                        if working.remove(&key).is_some() {
                            error!(
                                shopId = %key.shop_id,
                                shopsProductId = %key.shops_product_id,
                                "Product already exists. Cannot create."
                            );
                        }
                    }

                    let events: Vec<ProductEventRecord> = working
                        .into_values()
                        .map(|mut cmd| {
                            self.enrich_price(&mut cmd);
                            heuristics::classify_images(&mut cmd);
                            heuristics::enrich_origin_year(&mut cmd);
                            heuristics::enrich_authenticity(&mut cmd);
                            heuristics::enrich_condition(&mut cmd);
                            heuristics::enrich_provenance(&mut cmd);
                            heuristics::enrich_restoration(&mut cmd);
                            heuristics::classify_period(&mut cmd, &cache.period_keywords);
                            heuristics::classify_category(&mut cmd, &cache.category_keywords);
                            ProductEventRecord::Domain(ProductDomainEventRecord::from(
                                Product::create(
                                    cmd.shop_id,
                                    cmd.shops_product_id,
                                    cmd.shop_name,
                                    cmd.shop_type,
                                    cmd.native_title,
                                    cmd.native_description,
                                    cmd.native_price,
                                    cmd.other_price,
                                    cmd.native_price_estimate_min,
                                    cmd.other_price_estimate_min,
                                    cmd.native_price_estimate_max,
                                    cmd.other_price_estimate_max,
                                    cmd.state,
                                    cmd.url,
                                    cmd.images,
                                    cmd.auction_start,
                                    cmd.auction_end,
                                ),
                            ))
                        })
                        .collect();

                    let persist_failures = self.persist_events(events, &mut key_cmds).await;
                    failures.extend(persist_failures.into_iter().map(|(_, cmd)| cmd));
                }
                Err(err) => {
                    error!(err = ?err, "Failed entire BatchGetItem-Operation.");
                    failures.extend(working.into_values());
                }
            }
        }

        failures
    }

    async fn update(
        &self,
        cmds: HashMap<ProductKey, UpdateProductCommand>,
    ) -> HashMap<ProductKey, UpdateProductCommand> {
        let mut failures = HashMap::new();

        for chunk in
            Batch::<(ProductKey, UpdateProductCommand), 100>::chunked_from(cmds.into_iter())
        {
            let mut key_cmds: HashMap<ProductKey, UpdateProductCommand> =
                chunk.into_iter().collect();
            let mut working = key_cmds.clone();
            let keys: Batch<ProductKey, 100> = working
                .keys()
                .cloned()
                .collect::<Vec<_>>()
                .try_into()
                .expect("shouldn't fail because keys come from a Batch<_, 100>");

            match self.dynamodb_repository.get_product_records(&keys).await {
                Ok(records) => {
                    if let Some(unprocessed) = records.unprocessed {
                        for key in unprocessed {
                            if let Some(cmd) = working.remove(&key) {
                                failures.insert(key, cmd);
                            }
                        }
                    }

                    let events = determine_update_events(&mut working, records.items, self.fx_rate);
                    let events: Vec<ProductEventRecord> =
                        events.into_iter().map(ProductEventRecord::from).collect();

                    // Remaining items in `working` are products not found in DynamoDB —
                    // `determine_update_events` removes matched keys.
                    for (key, cmd) in &working {
                        error!(
                            shopId = %key.shop_id,
                            shopsProductId = %key.shops_product_id,
                            "Product not found. Cannot update."
                        );
                        failures.insert(key.clone(), cmd.clone());
                    }

                    let persist_failures = self.persist_events(events, &mut key_cmds).await;
                    failures.extend(persist_failures);
                }
                Err(err) => {
                    error!(err = ?err, "Failed entire BatchGetItem-Operation.");
                    failures.extend(working);
                }
            }
        }

        failures
    }

    async fn upsert(&self, cmds: Vec<UpsertProductCommand>) -> Vec<UpsertProductCommand> {
        let mut failures = Vec::new();
        let cache = self.classification_cache().await;

        for chunk in Batch::<UpsertProductCommand, 100>::chunked_from(cmds.into_iter()) {
            let mut key_cmds: HashMap<ProductKey, UpsertProductCommand> =
                chunk.into_iter().map(|cmd| (cmd.key(), cmd)).collect();
            let mut working = key_cmds.clone();
            let keys: Batch<ProductKey, 100> = working
                .keys()
                .cloned()
                .collect::<Vec<_>>()
                .try_into()
                .expect("shouldn't fail because keys come from a Batch<_, 100>");

            match self.dynamodb_repository.get_product_records(&keys).await {
                Ok(records) => {
                    if let Some(unprocessed) = records.unprocessed {
                        for key in unprocessed {
                            if let Some(cmd) = working.remove(&key) {
                                failures.push(cmd);
                            }
                        }
                    }

                    // Build update commands for existing products
                    let mut update_cmds: HashMap<ProductKey, UpdateProductCommand> = HashMap::new();
                    for record in &records.items {
                        let key = record.key();
                        if let Some(cmd) = working.remove(&key) {
                            update_cmds.insert(key, UpdateProductCommand::from(&cmd));
                        }
                    }

                    // Determine update events for existing products
                    let update_events =
                        determine_update_events(&mut update_cmds, records.items, self.fx_rate);

                    // Remaining items in `working` are products not found in DynamoDB → create
                    let create_events: Vec<ProductEventRecord> = working
                        .into_values()
                        .map(|cmd| {
                            let mut create_cmd = CreateProductCommand::from(cmd);
                            self.enrich_price(&mut create_cmd);
                            heuristics::classify_images(&mut create_cmd);
                            heuristics::enrich_origin_year(&mut create_cmd);
                            heuristics::enrich_authenticity(&mut create_cmd);
                            heuristics::enrich_condition(&mut create_cmd);
                            heuristics::enrich_provenance(&mut create_cmd);
                            heuristics::enrich_restoration(&mut create_cmd);
                            heuristics::classify_period(&mut create_cmd, &cache.period_keywords);
                            heuristics::classify_category(
                                &mut create_cmd,
                                &cache.category_keywords,
                            );
                            ProductEventRecord::Domain(ProductDomainEventRecord::from(
                                Product::create(
                                    create_cmd.shop_id,
                                    create_cmd.shops_product_id,
                                    create_cmd.shop_name,
                                    create_cmd.shop_type,
                                    create_cmd.native_title,
                                    create_cmd.native_description,
                                    create_cmd.native_price,
                                    create_cmd.other_price,
                                    create_cmd.native_price_estimate_min,
                                    create_cmd.other_price_estimate_min,
                                    create_cmd.native_price_estimate_max,
                                    create_cmd.other_price_estimate_max,
                                    create_cmd.state,
                                    create_cmd.url,
                                    create_cmd.images,
                                    create_cmd.auction_start,
                                    create_cmd.auction_end,
                                ),
                            ))
                        })
                        .collect();

                    let all_events: Vec<ProductEventRecord> = update_events
                        .into_iter()
                        .map(ProductEventRecord::from)
                        .chain(create_events)
                        .collect();

                    let persist_failures = self.persist_events(all_events, &mut key_cmds).await;
                    failures.extend(persist_failures.into_iter().map(|(_, cmd)| cmd));
                }
                Err(err) => {
                    error!(err = ?err, "Failed entire BatchGetItem-Operation.");
                    failures.extend(working.into_values());
                }
            }
        }

        failures
    }
}

fn determine_update_events(
    working: &mut HashMap<ProductKey, UpdateProductCommand>,
    records: Vec<impl HasKey<Key = ProductKey> + Into<Product>>,
    fx_rate: &impl FxRate,
) -> Vec<ProductDomainEventRecord> {
    let mut events = Vec::new();

    for record in records {
        let key = record.key();
        if let Some(cmd) = working.remove(&key) {
            let mut product = record.into();
            if let Some(price_event) = product.new_price(cmd.native_price, fx_rate) {
                events.push(ProductDomainEventRecord::from(price_event));
            }
            if let Some(new_state) = cmd.state
                && let Some(state_event) = product.change_state(new_state)
            {
                events.push(ProductDomainEventRecord::from(state_event));
            }
            if let Some(event) = product.change_estimate_price(
                cmd.native_price_estimate_min,
                cmd.native_price_estimate_max,
                fx_rate,
            ) {
                events.push(ProductDomainEventRecord::from(event));
            }
            if let Some(url) = cmd.url
                && let Some(event) = product.change_url(url)
            {
                events.push(ProductDomainEventRecord::from(event));
            }
            if let Some(images) = cmd.images
                && let Some(event) = product.change_images(images)
            {
                events.push(ProductDomainEventRecord::from(event));
            }
            if (cmd.auction_start.is_some() || cmd.auction_end.is_some())
                && let Some(event) = product.change_auction_time(cmd.auction_start, cmd.auction_end)
            {
                events.push(ProductDomainEventRecord::from(event));
            }
            if let Some(oy) = cmd.origin_year
                && let Some(event) = product.change_origin_year(oy)
            {
                events.push(ProductDomainEventRecord::from(event));
            }
            if let Some(auth) = cmd.authenticity
                && let Some(event) = product.change_authenticity(auth)
            {
                events.push(ProductDomainEventRecord::from(event));
            }
            if let Some(cond) = cmd.condition
                && let Some(event) = product.change_condition(cond)
            {
                events.push(ProductDomainEventRecord::from(event));
            }
            if let Some(prov) = cmd.provenance
                && let Some(event) = product.change_provenance(prov)
            {
                events.push(ProductDomainEventRecord::from(event));
            }
            if let Some(rest) = cmd.restoration
                && let Some(event) = product.change_restoration(rest)
            {
                events.push(ProductDomainEventRecord::from(event));
            }
        }
    }

    events
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::product::Product;
    use crate::dynamodb::repository::MockProductDynamoDbRepository;
    use crate::service::product_command::{CreateProductCommand, UpdateProductCommand};
    use aws_sdk_dynamodb::error::SdkError;
    use common::has_key::HasKey;
    use common::{price::domain::FixedFxRate, product_state::domain::ProductState};
    use fake::{Fake, Faker};
    use product_classification::category::service::MockCategoryService;
    use product_classification::period::service::MockPeriodService;
    use rstest;

    fn empty_period_service() -> MockPeriodService {
        let mut service = MockPeriodService::default();
        service
            .expect_find_periods()
            .returning(|| Box::pin(async { Ok(vec![]) }));
        service
    }

    fn empty_category_service() -> MockCategoryService {
        let mut service = MockCategoryService::default();
        service
            .expect_find_categories()
            .returning(|| Box::pin(async { Ok(vec![]) }));
        service
    }

    mod determine_update_events {
        use super::*;
        use crate::dynamodb::product_event_type_record::domain::ProductDomainEventTypeRecord;
        use crate::dynamodb::product_record::ProductRecord;
        use common::price::domain::Price;

        #[test]
        fn should_determine_no_update_events_when_only_skipped() {
            let record1 = Faker.fake::<ProductRecord>();
            let product1 = Product::from(record1.clone());
            let cmd1 = UpdateProductCommand {
                native_price: product1.native_price,
                state: Some(product1.state),
                native_price_estimate_min: None,
                native_price_estimate_max: None,
                url: None,
                images: None,
                auction_start: None,
                auction_end: None,
                origin_year: None,
                authenticity: None,
                condition: None,
                provenance: None,
                restoration: None,
            };

            let record2 = Faker.fake::<ProductRecord>();
            let product2 = Product::from(record2.clone());
            let cmd2 = UpdateProductCommand {
                native_price: product2.native_price,
                state: Some(product2.state),
                native_price_estimate_min: None,
                native_price_estimate_max: None,
                url: None,
                images: None,
                auction_start: None,
                auction_end: None,
                origin_year: None,
                authenticity: None,
                condition: None,
                provenance: None,
                restoration: None,
            };

            let mut working = HashMap::from([(product1.key(), cmd1), (product2.key(), cmd2)]);

            let actual =
                determine_update_events(&mut working, vec![record1, record2], &FixedFxRate());
            assert!(working.is_empty());
            assert!(actual.is_empty());
        }

        #[test]
        fn should_determine_update_events_when_none_skipped() {
            let record1 = Faker.fake::<ProductRecord>();
            let product1 = Product::from(record1.clone());
            let cmd1 = UpdateProductCommand {
                native_price: Some(Faker.fake()),
                state: Some(product1.state),
                native_price_estimate_min: None,
                native_price_estimate_max: None,
                url: None,
                images: None,
                auction_start: None,
                auction_end: None,
                origin_year: None,
                authenticity: None,
                condition: None,
                provenance: None,
                restoration: None,
            };

            let record2 = Faker.fake::<ProductRecord>();
            let product2 = Product::from(record2.clone());
            let cmd2 = UpdateProductCommand {
                native_price: Some(Faker.fake()),
                state: Some(if matches!(product2.state, ProductState::Available) {
                    ProductState::Removed
                } else {
                    ProductState::Available
                }),
                native_price_estimate_min: None,
                native_price_estimate_max: None,
                url: None,
                images: None,
                auction_start: None,
                auction_end: None,
                origin_year: None,
                authenticity: None,
                condition: None,
                provenance: None,
                restoration: None,
            };

            let mut working = HashMap::from([(product1.key(), cmd1), (product2.key(), cmd2)]);

            let actual =
                determine_update_events(&mut working, vec![record1, record2], &FixedFxRate());
            assert!(working.is_empty());
            assert_eq!(3, actual.len());
        }

        #[test]
        fn should_determine_update_events_when_some_skipped() {
            let record1 = Faker.fake::<ProductRecord>();
            let product1 = Product::from(record1.clone());
            let cmd1 = UpdateProductCommand {
                native_price: Some(Faker.fake()),
                state: Some(product1.state),
                native_price_estimate_min: None,
                native_price_estimate_max: None,
                url: None,
                images: None,
                auction_start: None,
                auction_end: None,
                origin_year: None,
                authenticity: None,
                condition: None,
                provenance: None,
                restoration: None,
            };

            let record2 = Faker.fake::<ProductRecord>();
            let product2 = Product::from(record2.clone());
            let cmd2 = UpdateProductCommand {
                native_price: product2.native_price,
                state: Some(product2.state),
                native_price_estimate_min: None,
                native_price_estimate_max: None,
                url: None,
                images: None,
                auction_start: None,
                auction_end: None,
                origin_year: None,
                authenticity: None,
                condition: None,
                provenance: None,
                restoration: None,
            };

            let mut working = HashMap::from([(product1.key(), cmd1), (product2.key(), cmd2)]);

            let actual =
                determine_update_events(&mut working, vec![record1, record2], &FixedFxRate());
            assert!(working.is_empty());
            assert_eq!(1, actual.len());
        }

        #[test]
        fn should_leave_unmatched_keys_in_working() {
            let product = Faker.fake::<Product>();
            let cmd = UpdateProductCommand {
                native_price: Some(Faker.fake()),
                state: Some(ProductState::Available),
                native_price_estimate_min: None,
                native_price_estimate_max: None,
                url: None,
                images: None,
                auction_start: None,
                auction_end: None,
                origin_year: None,
                authenticity: None,
                condition: None,
                provenance: None,
                restoration: None,
            };

            let mut working = HashMap::from([(product.key(), cmd.clone())]);

            let actual =
                determine_update_events(&mut working, Vec::<ProductRecord>::new(), &FixedFxRate());
            assert!(actual.is_empty());
            assert_eq!(1, working.len());
            assert_eq!(Some(&cmd), working.get(&product.key()));
        }

        #[test]
        fn should_generate_estimate_price_changed_event_when_estimate_price_changes() {
            let mut product: Product = Faker.fake();
            let key = product.key();
            product.native_price_estimate_min = None;
            product.native_price_estimate_max = None;
            let new_min = Some(Price::new(
                100u64.into(),
                common::currency::domain::Currency::Eur,
            ));
            let new_max = Some(Price::new(
                500u64.into(),
                common::currency::domain::Currency::Eur,
            ));
            let cmd = UpdateProductCommand {
                native_price: product.native_price,
                state: Some(product.state),
                native_price_estimate_min: new_min,
                native_price_estimate_max: new_max,
                url: None,
                images: None,
                auction_start: None,
                auction_end: None,
                origin_year: None,
                authenticity: None,
                condition: None,
                provenance: None,
                restoration: None,
            };
            let mut working = HashMap::from([(key.clone(), cmd)]);
            let events = determine_update_events(&mut working, vec![product], &FixedFxRate());
            assert!(
                events
                    .iter()
                    .any(|e| e.event_type
                        == ProductDomainEventTypeRecord::DomainEstimatePriceChanged)
            );
        }

        #[test]
        fn should_generate_url_changed_event_when_url_changes() {
            let mut product: Product = Faker.fake();
            let key = product.key();
            product.url = url::Url::parse("https://original.example.com").unwrap();
            let cmd = UpdateProductCommand {
                native_price: product.native_price,
                state: Some(product.state),
                native_price_estimate_min: None,
                native_price_estimate_max: None,
                url: Some(url::Url::parse("https://definitely-different.example.com").unwrap()),
                images: None,
                auction_start: None,
                auction_end: None,
                origin_year: None,
                authenticity: None,
                condition: None,
                provenance: None,
                restoration: None,
            };
            let mut working = HashMap::from([(key.clone(), cmd)]);
            let events = determine_update_events(&mut working, vec![product], &FixedFxRate());
            assert!(
                events
                    .iter()
                    .any(|e| e.event_type == ProductDomainEventTypeRecord::DomainUrlChanged)
            );
        }

        #[test]
        fn should_generate_images_changed_event_when_images_change() {
            use crate::core::product_image::ProductImage;
            use crate::core::prohibited_content::ProhibitedContent;

            let mut product: Product = Faker.fake();
            let key = product.key();
            product.images = vec![];
            let new_images = vec![ProductImage {
                url: url::Url::parse("https://img.example.com/new.jpg").unwrap(),
                prohibited_content: ProhibitedContent::None,
            }];
            let cmd = UpdateProductCommand {
                native_price: product.native_price,
                state: Some(product.state),
                native_price_estimate_min: None,
                native_price_estimate_max: None,
                url: None,
                images: Some(new_images),
                auction_start: None,
                auction_end: None,
                origin_year: None,
                authenticity: None,
                condition: None,
                provenance: None,
                restoration: None,
            };
            let mut working = HashMap::from([(key.clone(), cmd)]);
            let events = determine_update_events(&mut working, vec![product], &FixedFxRate());
            assert!(
                events
                    .iter()
                    .any(|e| e.event_type == ProductDomainEventTypeRecord::DomainImagesChanged)
            );
        }

        #[test]
        fn should_generate_auction_time_changed_event_when_auction_time_changes() {
            let mut product: Product = Faker.fake();
            let key = product.key();
            product.auction_start = None;
            product.auction_end = None;
            let cmd = UpdateProductCommand {
                native_price: product.native_price,
                state: Some(product.state),
                native_price_estimate_min: None,
                native_price_estimate_max: None,
                url: None,
                images: None,
                auction_start: Some(time::OffsetDateTime::now_utc() + time::Duration::days(30)),
                auction_end: None,
                origin_year: None,
                authenticity: None,
                condition: None,
                provenance: None,
                restoration: None,
            };
            let mut working = HashMap::from([(key.clone(), cmd)]);
            let events = determine_update_events(&mut working, vec![product], &FixedFxRate());
            assert!(events.iter().any(
                |e| e.event_type == ProductDomainEventTypeRecord::DomainAuctionTimeChanged
            ));
        }

        #[test]
        fn should_generate_origin_year_changed_event_when_origin_year_changes() {
            use crate::core::origin_year::OriginYear;

            let mut product: Product = Faker.fake();
            let key = product.key();
            product.origin_year = None;
            let cmd = UpdateProductCommand {
                native_price: product.native_price,
                state: Some(product.state),
                native_price_estimate_min: None,
                native_price_estimate_max: None,
                url: None,
                images: None,
                auction_start: None,
                auction_end: None,
                origin_year: Some(OriginYear::ExactYear(common::year::Year::from(1800i32))),
                authenticity: None,
                condition: None,
                provenance: None,
                restoration: None,
            };
            let mut working = HashMap::from([(key.clone(), cmd)]);
            let events = determine_update_events(&mut working, vec![product], &FixedFxRate());
            assert!(
                events
                    .iter()
                    .any(|e| e.event_type == ProductDomainEventTypeRecord::DomainOriginYearChanged)
            );
        }

        #[test]
        fn should_generate_authenticity_changed_event_when_authenticity_changes() {
            use crate::core::authenticity::Authenticity;

            let mut product: Product = Faker.fake();
            let key = product.key();
            product.authenticity = Authenticity::Unknown;
            let cmd = UpdateProductCommand {
                native_price: product.native_price,
                state: Some(product.state),
                native_price_estimate_min: None,
                native_price_estimate_max: None,
                url: None,
                images: None,
                auction_start: None,
                auction_end: None,
                origin_year: None,
                authenticity: Some(Authenticity::Original),
                condition: None,
                provenance: None,
                restoration: None,
            };
            let mut working = HashMap::from([(key.clone(), cmd)]);
            let events = determine_update_events(&mut working, vec![product], &FixedFxRate());
            assert!(
                events.iter().any(
                    |e| e.event_type == ProductDomainEventTypeRecord::DomainAuthenticityChanged
                )
            );
        }

        #[test]
        fn should_generate_condition_changed_event_when_condition_changes() {
            use crate::core::condition::Condition;

            let mut product: Product = Faker.fake();
            let key = product.key();
            product.condition = Condition::Unknown;
            let cmd = UpdateProductCommand {
                native_price: product.native_price,
                state: Some(product.state),
                native_price_estimate_min: None,
                native_price_estimate_max: None,
                url: None,
                images: None,
                auction_start: None,
                auction_end: None,
                origin_year: None,
                authenticity: None,
                condition: Some(Condition::Excellent),
                provenance: None,
                restoration: None,
            };
            let mut working = HashMap::from([(key.clone(), cmd)]);
            let events = determine_update_events(&mut working, vec![product], &FixedFxRate());
            assert!(
                events
                    .iter()
                    .any(|e| e.event_type == ProductDomainEventTypeRecord::DomainConditionChanged)
            );
        }

        #[test]
        fn should_generate_provenance_changed_event_when_provenance_changes() {
            use crate::core::provenance::Provenance;

            let mut product: Product = Faker.fake();
            let key = product.key();
            product.provenance = Provenance::Unknown;
            let cmd = UpdateProductCommand {
                native_price: product.native_price,
                state: Some(product.state),
                native_price_estimate_min: None,
                native_price_estimate_max: None,
                url: None,
                images: None,
                auction_start: None,
                auction_end: None,
                origin_year: None,
                authenticity: None,
                condition: None,
                provenance: Some(Provenance::Complete),
                restoration: None,
            };
            let mut working = HashMap::from([(key.clone(), cmd)]);
            let events = determine_update_events(&mut working, vec![product], &FixedFxRate());
            assert!(
                events
                    .iter()
                    .any(|e| e.event_type == ProductDomainEventTypeRecord::DomainProvenanceChanged)
            );
        }

        #[test]
        fn should_generate_restoration_changed_event_when_restoration_changes() {
            use crate::core::restoration::Restoration;

            let mut product: Product = Faker.fake();
            let key = product.key();
            product.restoration = Restoration::Unknown;
            let cmd = UpdateProductCommand {
                native_price: product.native_price,
                state: Some(product.state),
                native_price_estimate_min: None,
                native_price_estimate_max: None,
                url: None,
                images: None,
                auction_start: None,
                auction_end: None,
                origin_year: None,
                authenticity: None,
                condition: None,
                provenance: None,
                restoration: Some(Restoration::Minor),
            };
            let mut working = HashMap::from([(key.clone(), cmd)]);
            let events = determine_update_events(&mut working, vec![product], &FixedFxRate());
            assert!(events.iter().any(
                |e| e.event_type == ProductDomainEventTypeRecord::DomainRestorationChanged
            ));
        }

        #[test]
        fn should_generate_no_events_when_no_fields_change() {
            let product: Product = Faker.fake();
            let key = product.key();
            let cmd = UpdateProductCommand {
                native_price: product.native_price,
                state: Some(product.state),
                native_price_estimate_min: None,
                native_price_estimate_max: None,
                url: None,
                images: None,
                auction_start: None,
                auction_end: None,
                origin_year: None,
                authenticity: None,
                condition: None,
                provenance: None,
                restoration: None,
            };
            let mut working = HashMap::from([(key.clone(), cmd)]);
            let events = determine_update_events(&mut working, vec![product], &FixedFxRate());
            assert!(events.is_empty());
        }

        #[test]
        fn should_generate_multiple_events_when_multiple_fields_change() {
            use crate::core::authenticity::Authenticity;
            use crate::core::condition::Condition;

            let mut product: Product = Faker.fake();
            let key = product.key();
            product.url = url::Url::parse("https://original.example.com").unwrap();
            product.authenticity = Authenticity::Unknown;
            product.condition = Condition::Unknown;
            let cmd = UpdateProductCommand {
                native_price: product.native_price,
                state: Some(product.state),
                native_price_estimate_min: None,
                native_price_estimate_max: None,
                url: Some(url::Url::parse("https://different.example.com").unwrap()),
                images: None,
                auction_start: None,
                auction_end: None,
                origin_year: None,
                authenticity: Some(Authenticity::Original),
                condition: Some(Condition::Excellent),
                provenance: None,
                restoration: None,
            };
            let mut working = HashMap::from([(key.clone(), cmd)]);
            let events = determine_update_events(&mut working, vec![product], &FixedFxRate());
            assert!(events.len() >= 3);
            assert!(
                events
                    .iter()
                    .any(|e| e.event_type == ProductDomainEventTypeRecord::DomainUrlChanged)
            );
            assert!(
                events.iter().any(
                    |e| e.event_type == ProductDomainEventTypeRecord::DomainAuthenticityChanged
                )
            );
            assert!(
                events
                    .iter()
                    .any(|e| e.event_type == ProductDomainEventTypeRecord::DomainConditionChanged)
            );
        }
    }

    mod create {
        use super::*;
        use crate::dynamodb::product_record::ProductRecord;
        use common::batch::dynamodb::BatchGetItemResult;

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
        async fn should_fail_entire_chunk_when_batch_get_entirely_fails(
            #[case] expected: SdkError<
                aws_sdk_dynamodb::operation::batch_get_item::BatchGetItemError,
                aws_sdk_dynamodb::config::http::HttpResponse,
            >,
        ) {
            let mut repository = MockProductDynamoDbRepository::default();
            repository
                .expect_get_product_records()
                .return_once(|_| Box::pin(async { Err(expected) }));

            let period_service = empty_period_service();
            let category_service = empty_category_service();
            let service = CommandProductServiceImpl::new(
                &repository,
                &FixedFxRate(),
                &period_service,
                &category_service,
            );

            let mut expected = fake::vec![CreateProductCommand; 89];
            let mut actual = service.create(expected.clone()).await;

            expected.sort_by_key(|l| l.key());
            actual.sort_by_key(|l| l.key());

            assert_eq!(expected, actual);
        }

        #[tokio::test]
        async fn should_create_products_when_none_exist() {
            let mut repository = MockProductDynamoDbRepository::default();
            repository.expect_get_product_records().returning(|_| {
                Box::pin(async {
                    Ok(BatchGetItemResult {
                        items: vec![],
                        unprocessed: None,
                    })
                })
            });
            repository
                .expect_put_product_event_records()
                .returning(|_| {
                    Box::pin(async {
                        Ok(aws_sdk_dynamodb::operation::batch_write_item::BatchWriteItemOutput::builder().build())
                    })
                });

            let period_service = empty_period_service();
            let category_service = empty_category_service();
            let service = CommandProductServiceImpl::new(
                &repository,
                &FixedFxRate(),
                &period_service,
                &category_service,
            );
            let cmds = fake::vec![CreateProductCommand; 5];
            let failures = service.create(cmds).await;

            assert!(failures.is_empty());
        }

        #[tokio::test]
        async fn should_skip_products_that_already_exist() {
            let existing_record = Faker.fake::<ProductRecord>();
            let existing_key = existing_record.key();

            let mut existing_cmd = Faker.fake::<CreateProductCommand>();
            existing_cmd.shop_id = existing_key.shop_id;
            existing_cmd.shops_product_id = existing_key.shops_product_id;

            let mut repository = MockProductDynamoDbRepository::default();
            repository.expect_get_product_records().returning(move |_| {
                let record = existing_record.clone();
                Box::pin(async move {
                    Ok(BatchGetItemResult {
                        items: vec![record],
                        unprocessed: None,
                    })
                })
            });
            repository
                .expect_put_product_event_records()
                .returning(|_| {
                    Box::pin(async {
                        Ok(aws_sdk_dynamodb::operation::batch_write_item::BatchWriteItemOutput::builder().build())
                    })
                });

            let period_service = empty_period_service();
            let category_service = empty_category_service();
            let service = CommandProductServiceImpl::new(
                &repository,
                &FixedFxRate(),
                &period_service,
                &category_service,
            );

            let mut cmds = fake::vec![CreateProductCommand; 3];
            cmds.push(existing_cmd);
            let failures = service.create(cmds).await;

            assert!(failures.is_empty());
        }

        #[tokio::test]
        async fn should_return_unprocessed_as_failures() {
            let cmds = fake::vec![CreateProductCommand; 3];
            let unprocessed_keys: Vec<ProductKey> = cmds.iter().map(|c| c.key()).collect();

            let mut repository = MockProductDynamoDbRepository::default();
            repository.expect_get_product_records().returning(move |_| {
                let keys = unprocessed_keys.clone();
                Box::pin(async move {
                    Ok(BatchGetItemResult {
                        items: vec![],
                        unprocessed: Some(keys.try_into().unwrap()),
                    })
                })
            });

            let period_service = empty_period_service();
            let category_service = empty_category_service();
            let service = CommandProductServiceImpl::new(
                &repository,
                &FixedFxRate(),
                &period_service,
                &category_service,
            );

            let mut expected = cmds.clone();
            let mut actual = service.create(cmds).await;

            expected.sort_by_key(|l| l.key());
            actual.sort_by_key(|l| l.key());

            assert_eq!(expected, actual);
        }

        #[tokio::test]
        async fn should_enrich_prices_for_created_products() {
            use common::currency::domain::Currency;
            use strum::EnumCount;

            let mut cmd = Faker.fake::<CreateProductCommand>();
            cmd.native_price = Some(Faker.fake());
            cmd.other_price.clear();
            cmd.native_price_estimate_min = Some(Faker.fake());
            cmd.other_price_estimate_min.clear();
            cmd.native_price_estimate_max = Some(Faker.fake());
            cmd.other_price_estimate_max.clear();

            let mut repository = MockProductDynamoDbRepository::default();
            repository.expect_get_product_records().returning(|_| {
                Box::pin(async {
                    Ok(BatchGetItemResult {
                        items: vec![],
                        unprocessed: None,
                    })
                })
            });
            repository
                .expect_put_product_event_records()
                .returning(|_| {
                    Box::pin(async {
                        Ok(aws_sdk_dynamodb::operation::batch_write_item::BatchWriteItemOutput::builder().build())
                    })
                });

            let period_service = empty_period_service();
            let category_service = empty_category_service();
            let service = CommandProductServiceImpl::new(
                &repository,
                &FixedFxRate(),
                &period_service,
                &category_service,
            );

            // Verify enrich_price directly
            let mut test_cmd = cmd.clone();
            service.enrich_price(&mut test_cmd);
            assert_eq!(Currency::COUNT, test_cmd.other_price.len());
            assert_eq!(Currency::COUNT, test_cmd.other_price_estimate_min.len());
            assert_eq!(Currency::COUNT, test_cmd.other_price_estimate_max.len());

            let failures = service.create(vec![cmd]).await;
            assert!(failures.is_empty());
        }
    }

    mod update {
        use super::*;
        use crate::dynamodb::product_record::ProductRecord;
        use common::batch::dynamodb::BatchGetItemResult;

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
        async fn should_fail_entire_chunk_when_batch_get_entirely_fails(
            #[case] expected: SdkError<
                aws_sdk_dynamodb::operation::batch_get_item::BatchGetItemError,
                aws_sdk_dynamodb::config::http::HttpResponse,
            >,
        ) {
            let mut repository = MockProductDynamoDbRepository::default();
            repository
                .expect_get_product_records()
                .return_once(|_| Box::pin(async { Err(expected) }));

            let period_service = empty_period_service();
            let category_service = empty_category_service();
            let service = CommandProductServiceImpl::new(
                &repository,
                &FixedFxRate(),
                &period_service,
                &category_service,
            );

            let cmds: HashMap<ProductKey, UpdateProductCommand> = (0..5)
                .map(|_| {
                    let product = Faker.fake::<Product>();
                    (product.key(), Faker.fake())
                })
                .collect();
            let expected_keys: Vec<ProductKey> = cmds.keys().cloned().collect();
            let actual = service.update(cmds).await;

            let mut actual_keys: Vec<ProductKey> = actual.keys().cloned().collect();
            let mut expected_sorted = expected_keys;
            expected_sorted.sort();
            actual_keys.sort();

            assert_eq!(expected_sorted, actual_keys);
        }

        #[tokio::test]
        async fn should_update_products_when_all_exist() {
            let record = Faker.fake::<ProductRecord>();
            let product = Product::from(record.clone());
            let key = product.key();
            let cmd = UpdateProductCommand {
                native_price: Some(Faker.fake()),
                state: Some(if matches!(product.state, ProductState::Available) {
                    ProductState::Removed
                } else {
                    ProductState::Available
                }),
                native_price_estimate_min: None,
                native_price_estimate_max: None,
                url: None,
                images: None,
                auction_start: None,
                auction_end: None,
                origin_year: None,
                authenticity: None,
                condition: None,
                provenance: None,
                restoration: None,
            };

            let mut repository = MockProductDynamoDbRepository::default();
            repository.expect_get_product_records().returning(move |_| {
                let r = record.clone();
                Box::pin(async move {
                    Ok(BatchGetItemResult {
                        items: vec![r],
                        unprocessed: None,
                    })
                })
            });
            repository
                .expect_put_product_event_records()
                .returning(|_| {
                    Box::pin(async {
                        Ok(aws_sdk_dynamodb::operation::batch_write_item::BatchWriteItemOutput::builder().build())
                    })
                });

            let period_service = empty_period_service();
            let category_service = empty_category_service();
            let service = CommandProductServiceImpl::new(
                &repository,
                &FixedFxRate(),
                &period_service,
                &category_service,
            );
            let failures = service.update(HashMap::from([(key, cmd)])).await;

            assert!(failures.is_empty());
        }

        #[tokio::test]
        async fn should_return_not_found_products_as_failures() {
            let mut repository = MockProductDynamoDbRepository::default();
            repository.expect_get_product_records().returning(|_| {
                Box::pin(async {
                    Ok(BatchGetItemResult {
                        items: vec![],
                        unprocessed: None,
                    })
                })
            });

            let period_service = empty_period_service();
            let category_service = empty_category_service();
            let service = CommandProductServiceImpl::new(
                &repository,
                &FixedFxRate(),
                &period_service,
                &category_service,
            );

            let cmds: HashMap<ProductKey, UpdateProductCommand> = (0..3)
                .map(|_| {
                    let product = Faker.fake::<Product>();
                    (product.key(), Faker.fake())
                })
                .collect();
            let expected_keys: Vec<ProductKey> = cmds.keys().cloned().collect();
            let actual = service.update(cmds).await;

            let mut actual_keys: Vec<ProductKey> = actual.keys().cloned().collect();
            let mut expected_sorted = expected_keys;
            expected_sorted.sort();
            actual_keys.sort();

            assert_eq!(expected_sorted, actual_keys);
        }

        #[tokio::test]
        async fn should_return_unprocessed_as_failures() {
            let product = Faker.fake::<Product>();
            let key = product.key();
            let cmd: UpdateProductCommand = Faker.fake();

            let unprocessed_key = key.clone();
            let mut repository = MockProductDynamoDbRepository::default();
            repository.expect_get_product_records().returning(move |_| {
                let k = unprocessed_key.clone();
                Box::pin(async move {
                    Ok(BatchGetItemResult {
                        items: vec![],
                        unprocessed: Some(vec![k].try_into().unwrap()),
                    })
                })
            });

            let period_service = empty_period_service();
            let category_service = empty_category_service();
            let service = CommandProductServiceImpl::new(
                &repository,
                &FixedFxRate(),
                &period_service,
                &category_service,
            );

            let actual = service
                .update(HashMap::from([(key.clone(), cmd.clone())]))
                .await;

            assert_eq!(1, actual.len());
            assert_eq!(Some(&cmd), actual.get(&key));
        }

        #[tokio::test]
        async fn should_skip_updates_when_no_changes() {
            let record = Faker.fake::<ProductRecord>();
            let product = Product::from(record.clone());
            let key = product.key();
            let cmd = UpdateProductCommand {
                native_price: product.native_price,
                state: Some(product.state),
                native_price_estimate_min: None,
                native_price_estimate_max: None,
                url: None,
                images: None,
                auction_start: None,
                auction_end: None,
                origin_year: None,
                authenticity: None,
                condition: None,
                provenance: None,
                restoration: None,
            };

            let mut repository = MockProductDynamoDbRepository::default();
            repository.expect_get_product_records().returning(move |_| {
                let r = record.clone();
                Box::pin(async move {
                    Ok(BatchGetItemResult {
                        items: vec![r],
                        unprocessed: None,
                    })
                })
            });
            // put_product_event_records should NOT be called since there are no events
            repository.expect_put_product_event_records().never();

            let period_service = empty_period_service();
            let category_service = empty_category_service();
            let service = CommandProductServiceImpl::new(
                &repository,
                &FixedFxRate(),
                &period_service,
                &category_service,
            );
            let failures = service.update(HashMap::from([(key, cmd)])).await;

            assert!(failures.is_empty());
        }
    }
}

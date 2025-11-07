use crate::{
    core::{item::LocalizedItemView, user_state::WatchlistUserState},
    watchlist::{
        dynamodb::repository::WatchlistItemDynamoDbRepository,
        service::item_watchlist_service::WatchItemError,
    },
};
use common::{personalized::Personalized, user_id::UserId};
use std::collections::HashMap;

#[derive(Debug, thiserror::Error)]
pub enum ItemPersonalizationError {
    #[error("WatchItemError: {0}")]
    WatchItemError(#[from] WatchItemError),
}

#[async_trait::async_trait]
#[mockall::automock]
pub trait ItemPersonalizationService {
    async fn personalize_watchlist(
        &self,
        user_id: &UserId,
        item: LocalizedItemView,
    ) -> Result<Personalized<LocalizedItemView, WatchlistUserState>, WatchItemError>;

    async fn personalize_all_watchlist(
        &self,
        user_id: &UserId,
        items: Vec<LocalizedItemView>,
    ) -> Result<Vec<Personalized<LocalizedItemView, WatchlistUserState>>, WatchItemError>;
}

pub struct ItemPersonalizationServiceImpl<'a> {
    watchlist_repository: &'a (dyn WatchlistItemDynamoDbRepository + Sync),
}

impl<'a> ItemPersonalizationServiceImpl<'a> {
    pub fn new(watchlist_repository: &'a (dyn WatchlistItemDynamoDbRepository + Sync)) -> Self {
        Self {
            watchlist_repository,
        }
    }
}

#[async_trait::async_trait]
impl<'a> ItemPersonalizationService for ItemPersonalizationServiceImpl<'a> {
    async fn personalize_watchlist(
        &self,
        user_id: &UserId,
        item: LocalizedItemView,
    ) -> Result<Personalized<LocalizedItemView, WatchlistUserState>, WatchItemError> {
        let watchlist_record = self
            .watchlist_repository
            .get_watchlist_record(user_id, &item.shop_id, &item.shops_item_id)
            .await?;

        let personalized = match watchlist_record {
            None => Personalized {
                item,
                user_state: Some(WatchlistUserState {
                    watching: false,
                    notifications: false,
                }),
            },
            Some(record) => Personalized {
                item,
                user_state: Some(WatchlistUserState {
                    watching: true,
                    notifications: record.notifications,
                }),
            },
        };
        Ok(personalized)
    }

    async fn personalize_all_watchlist(
        &self,
        user_id: &UserId,
        items: Vec<LocalizedItemView>,
    ) -> Result<Vec<Personalized<LocalizedItemView, WatchlistUserState>>, WatchItemError> {
        let watchlist_records = self
            .watchlist_repository
            .query_watchlist_records_all(user_id, true)
            .await?
            .into_iter()
            .map(|watchlist_record| (watchlist_record.item_id, watchlist_record))
            .collect::<HashMap<_, _>>();

        let personalized_items = items
            .into_iter()
            .map(|item| {
                let user_state = watchlist_records
                    .get(&item.item_id)
                    .map(|watchlist_record| WatchlistUserState {
                        watching: true,
                        notifications: watchlist_record.notifications,
                    })
                    .unwrap_or(WatchlistUserState {
                        watching: false,
                        notifications: false,
                    });
                Personalized {
                    item,
                    user_state: Some(user_state),
                }
            })
            .collect();

        Ok(personalized_items)
    }
}

#[cfg(test)]
mod tests {
    use crate::{
        core::item::LocalizedItemView,
        service::personalization_service::{
            ItemPersonalizationService, ItemPersonalizationServiceImpl,
        },
        watchlist::dynamodb::{
            record::WatchlistItemRecord, repository::MockWatchlistItemDynamoDbRepository,
        },
    };
    use common::item_id::ItemId;
    use fake::{Fake, Faker};
    use time::OffsetDateTime;

    #[tokio::test]
    async fn should_personalize_watchlist_when_watching_notifications_false() {
        let item_id = ItemId::new();
        let mut watchlist_repository = MockWatchlistItemDynamoDbRepository::default();
        watchlist_repository
            .expect_get_watchlist_record()
            .return_once(move |user_id, _, _| {
                let user_id = *user_id;
                Box::pin(async move {
                    let watched = WatchlistItemRecord {
                        shop_id: Faker.fake(),
                        shops_item_id: Faker.fake(),
                        item_id,
                        notifications: false,
                        created: OffsetDateTime::now_utc(),
                        updated: OffsetDateTime::now_utc(),
                        pk: "dummy".to_owned(),
                        sk: "dummy".to_owned(),
                        lsi1_sk: "dummy".to_owned(),
                        gsi1_pk: None,
                        gsi1_sk: None,
                        user_id,
                        user_record: Faker.fake(),
                    };
                    Ok(Some(watched))
                })
            });

        let service = ItemPersonalizationServiceImpl::new(&watchlist_repository);

        let mut input = Faker.fake::<LocalizedItemView>();
        input.item_id = item_id;

        let actual = service
            .personalize_watchlist(&Faker.fake(), input)
            .await
            .unwrap();

        assert!(actual.user_state.unwrap().watching);
        assert!(!actual.user_state.unwrap().notifications);
    }

    #[tokio::test]
    async fn should_personalize_watchlist_when_watching_notifications_true() {
        let item_id = ItemId::new();
        let mut watchlist_repository = MockWatchlistItemDynamoDbRepository::default();
        watchlist_repository
            .expect_get_watchlist_record()
            .return_once(move |user_id, _, _| {
                let user_id = *user_id;
                Box::pin(async move {
                    let watched = WatchlistItemRecord {
                        shop_id: Faker.fake(),
                        shops_item_id: Faker.fake(),
                        item_id,
                        notifications: true,
                        created: OffsetDateTime::now_utc(),
                        updated: OffsetDateTime::now_utc(),
                        pk: "dummy".to_owned(),
                        sk: "dummy".to_owned(),
                        lsi1_sk: "dummy".to_owned(),
                        gsi1_pk: None,
                        gsi1_sk: None,
                        user_id,
                        user_record: Faker.fake(),
                    };
                    Ok(Some(watched))
                })
            });

        let service = ItemPersonalizationServiceImpl::new(&watchlist_repository);

        let mut input = Faker.fake::<LocalizedItemView>();
        input.item_id = item_id;

        let actual = service
            .personalize_watchlist(&Faker.fake(), input)
            .await
            .unwrap();

        assert!(actual.user_state.unwrap().watching);
        assert!(actual.user_state.unwrap().notifications);
    }

    #[tokio::test]
    async fn should_personalize_watchlist_when_not_watching() {
        let item_id = ItemId::new();
        let mut watchlist_repository = MockWatchlistItemDynamoDbRepository::default();
        watchlist_repository
            .expect_get_watchlist_record()
            .return_once(move |_, _, _| Box::pin(async move { Ok(None) }));

        let service = ItemPersonalizationServiceImpl::new(&watchlist_repository);

        let mut input = Faker.fake::<LocalizedItemView>();
        input.item_id = item_id;

        let actual = service
            .personalize_watchlist(&Faker.fake(), input)
            .await
            .unwrap();

        assert!(!actual.user_state.unwrap().watching);
        assert!(!actual.user_state.unwrap().notifications);
    }

    #[tokio::test]
    async fn should_personalize_watchlist_all() {
        let item1_id = ItemId::new();
        let item2_id = ItemId::new();
        let item3_id = ItemId::new();
        let mut watchlist_repository = MockWatchlistItemDynamoDbRepository::default();
        watchlist_repository
            .expect_query_watchlist_records_all()
            .return_once(move |user_id, _| {
                let user_id = *user_id;
                Box::pin(async move {
                    let watched = vec![
                        WatchlistItemRecord {
                            shop_id: Faker.fake(),
                            shops_item_id: Faker.fake(),
                            item_id: item1_id,
                            notifications: false,
                            created: OffsetDateTime::now_utc(),
                            updated: OffsetDateTime::now_utc(),
                            pk: "dummy".to_owned(),
                            sk: "dummy".to_owned(),
                            lsi1_sk: "dummy".to_owned(),
                            gsi1_pk: None,
                            gsi1_sk: None,
                            user_id,
                            user_record: Faker.fake(),
                        },
                        WatchlistItemRecord {
                            shop_id: Faker.fake(),
                            shops_item_id: Faker.fake(),
                            item_id: item2_id,
                            notifications: true,
                            created: OffsetDateTime::now_utc(),
                            updated: OffsetDateTime::now_utc(),
                            pk: "dummy".to_owned(),
                            sk: "dummy".to_owned(),
                            lsi1_sk: "dummy".to_owned(),
                            gsi1_pk: None,
                            gsi1_sk: None,
                            user_id,
                            user_record: Faker.fake(),
                        },
                        WatchlistItemRecord {
                            shop_id: Faker.fake(),
                            shops_item_id: Faker.fake(),
                            item_id: item3_id,
                            notifications: true,
                            created: OffsetDateTime::now_utc(),
                            updated: OffsetDateTime::now_utc(),
                            pk: "dummy".to_owned(),
                            sk: "dummy".to_owned(),
                            lsi1_sk: "dummy".to_owned(),
                            gsi1_pk: None,
                            gsi1_sk: None,
                            user_id,
                            user_record: Faker.fake(),
                        },
                    ];
                    Ok(watched)
                })
            });

        let service = ItemPersonalizationServiceImpl::new(&watchlist_repository);

        let mut watched_in1 = Faker.fake::<LocalizedItemView>();
        watched_in1.item_id = item1_id;
        let mut watched_in2 = Faker.fake::<LocalizedItemView>();
        watched_in2.item_id = item2_id;
        let mut watched_in3 = Faker.fake::<LocalizedItemView>();
        watched_in3.item_id = item3_id;

        let input = vec![
            watched_in1,
            Faker.fake(),
            Faker.fake(),
            Faker.fake(),
            watched_in2,
            Faker.fake(),
            watched_in3,
        ];
        let actual = service
            .personalize_all_watchlist(&Faker.fake(), input)
            .await
            .unwrap();

        assert!(
            !actual[0].user_state.unwrap().notifications && actual[0].user_state.unwrap().watching
        );
        assert!(
            !actual[1].user_state.unwrap().notifications && !actual[1].user_state.unwrap().watching
        );
        assert!(
            !actual[2].user_state.unwrap().notifications && !actual[2].user_state.unwrap().watching
        );
        assert!(
            !actual[3].user_state.unwrap().notifications && !actual[3].user_state.unwrap().watching
        );
        assert!(
            actual[4].user_state.unwrap().notifications && actual[4].user_state.unwrap().watching
        );
        assert!(
            !actual[5].user_state.unwrap().notifications && !actual[5].user_state.unwrap().watching
        );
        assert!(
            actual[6].user_state.unwrap().notifications && actual[6].user_state.unwrap().watching
        );
    }
}

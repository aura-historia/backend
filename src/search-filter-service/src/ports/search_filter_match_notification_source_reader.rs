use common::error::boxed::BoxError;
use common::event_id::EventId;
use common::product_id::ProductId;
use common::user_id::UserId;
use common::user_search_filter_id::UserSearchFilterId;
use common::user_search_filter_name::UserSearchFilterName;

#[derive(Debug, Clone, PartialEq)]
pub struct SearchFilterMatchNotificationSource {
    pub user_id: UserId,
    pub search_filter_id: UserSearchFilterId,
    pub search_filter_name: UserSearchFilterName,
    pub product_id: ProductId,
    pub origin_event_id: EventId,
    pub external: bool,
}

#[derive(Debug, thiserror::Error)]
pub enum SearchFilterMatchNotificationSourceReadError {
    #[error("search filter match notification source read failed")]
    ReadFailed {
        #[source]
        source: BoxError,
    },
    #[error("search filter match notification source persisted state is invalid")]
    InvalidPersistedState {
        #[source]
        source: BoxError,
    },
}

#[async_trait::async_trait]
pub trait SearchFilterMatchNotificationSourceReader: Send {
    async fn find_source(
        &mut self,
        user_id: UserId,
        search_filter_id: UserSearchFilterId,
        product_id: ProductId,
    ) -> Result<
        Option<SearchFilterMatchNotificationSource>,
        SearchFilterMatchNotificationSourceReadError,
    >;
}

pub trait SearchFilterMatchNotificationSourceReaderFactory<Tx>: Send + Sync {
    fn in_transaction<'tx>(
        &'tx self,
        tx: &'tx mut Tx,
    ) -> impl SearchFilterMatchNotificationSourceReader + 'tx;
}

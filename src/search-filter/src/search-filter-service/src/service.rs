use crate::user_search_filter::UserSearchFilter;
use aws_sdk_dynamodb::{config::http::HttpResponse, error::SdkError};
use common::{sort::SortOrder, user_id::UserId};
use search_filter_core::{search_filter::SearchFilter, search_filter_id::SearchFilterId};
use search_filter_dynamodb::repository::SearchFilterDynamoDbRepository;
use time::OffsetDateTime;

#[derive(thiserror::Error, Debug)]
pub enum SearchFilterError {
    #[error("SearchFilter with UserId '{0}' and SearchFilterId '{1}' not found.")]
    SearchFilterNotFound(UserId, SearchFilterId),

    #[error("Encountered DynamoDB SdkError for GetItem: {0}")]
    SdkGetItemError(
        #[from] SdkError<aws_sdk_dynamodb::operation::get_item::GetItemError, HttpResponse>,
    ),

    #[error("Encountered DynamoDB SdkError for QueryItem: {0}")]
    SdkQueryError(#[from] SdkError<aws_sdk_dynamodb::operation::query::QueryError, HttpResponse>),

    #[error("Encountered DynamoDB SdkError for PutItem: {0}")]
    SdkPutItemError(
        #[from] SdkError<aws_sdk_dynamodb::operation::put_item::PutItemError, HttpResponse>,
    ),
}

#[cfg(feature = "api")]
pub mod api {
    use crate::service::SearchFilterError;
    use common::api::error::ApiError;
    use common::api::error_code::SEARCH_FILTER_NOT_FOUND;
    use tracing::error;

    impl From<SearchFilterError> for ApiError {
        fn from(err: SearchFilterError) -> Self {
            match err {
                SearchFilterError::SearchFilterNotFound(_, _) => {
                    ApiError::not_found(SEARCH_FILTER_NOT_FOUND)
                }
                SearchFilterError::SdkGetItemError(err) => {
                    error!(error = ?err, "Encountered SdkGetItemError while getting search-filter.");
                    err.into()
                }
                SearchFilterError::SdkQueryError(err) => {
                    error!(error = ?err, "Encountered SdkQueryError while querying search-filters.");
                    err.into()
                }
                SearchFilterError::SdkPutItemError(err) => {
                    error!(error = ?err, "Encountered SdkPutItemError while saving search-filter.");
                    err.into()
                }
            }
        }
    }
}

#[mockall::automock]
#[async_trait::async_trait]
pub trait SearchFilterService {
    async fn find_search_filters(
        &self,
        user_id: &UserId,
        sort_by_created: &SortOrder,
    ) -> Result<Vec<UserSearchFilter>, SearchFilterError>;

    async fn find_search_filter(
        &self,
        user_id: &UserId,
        search_filter_id: &SearchFilterId,
    ) -> Result<UserSearchFilter, SearchFilterError>;

    async fn save_search_filter(
        &self,
        user_id: &UserId,
        search_filter: SearchFilter,
    ) -> Result<UserSearchFilter, SearchFilterError>;

    async fn delete_search_filter(
        &self,
        user_id: &UserId,
        search_filter_id: &SearchFilterId,
    ) -> Result<(), SearchFilterError>;
}

pub struct SearchFilterServiceImpl<'a> {
    repository: &'a (dyn SearchFilterDynamoDbRepository + Sync),
}

impl<'a> SearchFilterServiceImpl<'a> {
    pub fn new(repository: &'a (dyn SearchFilterDynamoDbRepository + Sync)) -> Self {
        Self { repository }
    }
}

#[async_trait::async_trait]
impl<'a> SearchFilterService for SearchFilterServiceImpl<'a> {
    async fn find_search_filters(
        &self,
        user_id: &UserId,
        sort_by_created: &SortOrder,
    ) -> Result<Vec<UserSearchFilter>, SearchFilterError> {
        let search_filters = self
            .repository
            .query_search_filter_records(user_id, matches!(sort_by_created, SortOrder::Asc))
            .await?
            .into_iter()
            .map(UserSearchFilter::from)
            .collect();
        Ok(search_filters)
    }

    async fn find_search_filter(
        &self,
        user_id: &UserId,
        search_filter_id: &SearchFilterId,
    ) -> Result<UserSearchFilter, SearchFilterError> {
        let search_filter = self
            .repository
            .get_search_filter_record(user_id, search_filter_id)
            .await?
            .map(UserSearchFilter::from)
            .ok_or_else(|| SearchFilterError::SearchFilterNotFound(*user_id, *search_filter_id))?;
        Ok(search_filter)
    }

    async fn save_search_filter(
        &self,
        user_id: &UserId,
        search_filter: SearchFilter,
    ) -> Result<UserSearchFilter, SearchFilterError> {
        let user_search_filter = UserSearchFilter {
            user_id: *user_id,
            search_filter_id: SearchFilterId::new(),
            search_filter,
            created: OffsetDateTime::now_utc(),
            updated: OffsetDateTime::now_utc(),
        };

        let _ = self
            .repository
            .put_search_filter_record(user_search_filter.clone().into())
            .await?;

        Ok(user_search_filter)
    }

    async fn delete_search_filter(
        &self,
        user_id: &UserId,
        search_filter_id: &SearchFilterId,
    ) -> Result<(), SearchFilterError> {
        // exists guard
        let _ = self.find_search_filter(user_id, search_filter_id).await?;
        self.delete_search_filter(user_id, search_filter_id).await
    }
}

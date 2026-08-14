use crate::core::command::UpdateUserSearchFilterMatchCommand;
use crate::core::quota::SearchFilterQuota;
use crate::core::search_filter_product_match::SearchFilterProductMatch;
use crate::core::sort_search_filter_match_field::SortSearchFilterMatchField;
use crate::core::user_search_filter::UserSearchFilter;
use crate::core::user_search_filter_name::UserSearchFilterName;
use crate::core::user_search_filter_search::UserSearchFilterSearch;
use crate::core::user_search_filter_update::UserSearchFilterUpdate;
use crate::dynamodb::repository::UserSearchFilterDynamoDbRepository;
use crate::dynamodb::user_search_filter_match_record::UserSearchFilterMatchRecord;
use crate::dynamodb::user_search_filter_match_record_update::UserSearchFilterMatchRecordUpdate;
use aws_sdk_dynamodb::{config::http::HttpResponse, error::SdkError};
use common::actor::RequestContext;
use common::batch::Batch;
use common::pagination::cursor::{Cursor, CursoredResult};
use common::resource_state::domain::ResourceState;
use common::shop_id::ShopId;
use common::shops_product_id::ShopsProductId;
use common::sort::Sort;
use common::user_search_filter_id::UserSearchFilterId;
use common::{sort::SortOrder, user_id::UserId};
use embedding::{EmbeddingError, EmbeddingGenerator, EmbeddingText};
use product::core::{
    product::Product,
    product_search::{ProductSearch, ProductSearchSerdeField},
};
use product::opensearch::product_document::ProductDocument;
use time::OffsetDateTime;
use tracing::{error, info, warn};
use user::core::user::User;
use user::service::user_service::{UserService, UserServiceError};

#[derive(thiserror::Error, Debug)]
pub enum UserSearchFilterError {
    #[error("UserSearchFilter with UserId '{0}' and SearchFilterId '{1}' not found.")]
    UserSearchFilterNotFound(UserId, UserSearchFilterId),

    #[error(
        "UserSearchFilterMatch with UserId '{0}', SearchFilterId '{1}', ShopId '{2}', and ShopsProductId '{3}' not found."
    )]
    UserSearchFilterMatchNotFound(UserId, UserSearchFilterId, ShopId, ShopsProductId),

    #[error("Encountered DynamoDB SdkError for GetItem: {0:?}")]
    SdkGetItemError(
        #[from] SdkError<aws_sdk_dynamodb::operation::get_item::GetItemError, HttpResponse>,
    ),

    #[error("Encountered DynamoDB SdkError for QueryItem: {0:?}")]
    SdkQueryError(#[from] SdkError<aws_sdk_dynamodb::operation::query::QueryError, HttpResponse>),

    #[error("Encountered DynamoDB SdkError for PutItem: {0:?}")]
    SdkPutItemError(
        #[from] SdkError<aws_sdk_dynamodb::operation::put_item::PutItemError, HttpResponse>,
    ),

    #[error("Encountered DynamoDB SdkError for DeleteItem: {0:?}")]
    SdkDeleteItemError(
        #[from] SdkError<aws_sdk_dynamodb::operation::delete_item::DeleteItemError, HttpResponse>,
    ),

    #[error("Encountered DynamoDB SdkError for UpdateItem: {0:?}")]
    SdkUpdateItemError(
        #[from] SdkError<aws_sdk_dynamodb::operation::update_item::UpdateItemError, HttpResponse>,
    ),

    #[error("Encountered DynamoDB SdkError for BatchWriteItem: {0:?}")]
    SdkBatchWriteItemError(
        #[from]
        SdkError<aws_sdk_dynamodb::operation::batch_write_item::BatchWriteItemError, HttpResponse>,
    ),

    #[cfg(feature = "opensearch")]
    #[error("Encountered OpenSearch error: {0}")]
    OpenSearchError(#[from] opensearch::Error),

    #[error("Encountered embedding service error: {0}")]
    EmbeddingError(#[from] EmbeddingError),

    #[error("User with UserId '{0}' not found.")]
    UserNotFound(UserId),

    #[error(
        "Exceeded the maximum amount of search filters. There are already {0}/{1} search filters occupied."
    )]
    SearchFilterQuotaExceeded(u32, u32),

    #[error(
        "Search filter contains forbidden search field '{0}' which requires a higher user tier."
    )]
    SearchFilterFeatureForbidden(ProductSearchSerdeField),

    #[error("UserServiceError: {0}")]
    UserServiceError(UserServiceError),

    #[error("Periodic hybrid match batch write incomplete: {0} match(es) not persisted.")]
    PeriodicHybridMatchWriteIncomplete(usize),
}

#[cfg(feature = "data")]
pub mod api {
    use crate::service::user_search_filter_service::UserSearchFilterError;
    use common::api::error::ApiError;
    use common::api::error_code::{
        INTERNAL_SERVER_ERROR, SEARCH_FILTER_NOT_FOUND, SEARCH_FILTER_QUOTA_EXCEEDED,
        SEARCH_FILTER_RESTRICTED_FEATURE, USER_NOT_FOUND,
    };

    impl From<UserSearchFilterError> for ApiError {
        fn from(err: UserSearchFilterError) -> Self {
            match err {
                UserSearchFilterError::UserSearchFilterNotFound(_, _)
                | UserSearchFilterError::UserSearchFilterMatchNotFound(_, _, _, _) => {
                    ApiError::not_found(SEARCH_FILTER_NOT_FOUND, Box::new(err))
                }
                UserSearchFilterError::SdkGetItemError(err) => err.into(),
                UserSearchFilterError::SdkQueryError(err) => err.into(),
                UserSearchFilterError::SdkPutItemError(err) => err.into(),
                UserSearchFilterError::SdkDeleteItemError(err) => err.into(),
                UserSearchFilterError::SdkUpdateItemError(err) => err.into(),
                UserSearchFilterError::SdkBatchWriteItemError(err) => {
                    ApiError::internal_server_error(INTERNAL_SERVER_ERROR, Box::new(err))
                }
                #[cfg(feature = "opensearch")]
                UserSearchFilterError::OpenSearchError(err) => {
                    ApiError::internal_server_error(INTERNAL_SERVER_ERROR, Box::new(err))
                }
                UserSearchFilterError::EmbeddingError(err) => {
                    ApiError::internal_server_error(INTERNAL_SERVER_ERROR, Box::new(err))
                }
                UserSearchFilterError::UserNotFound(_) => {
                    ApiError::not_found(USER_NOT_FOUND, Box::new(err))
                }
                err @ UserSearchFilterError::SearchFilterQuotaExceeded(_, _) => {
                    let detail = err.to_string();
                    ApiError::unprocessable_entity(SEARCH_FILTER_QUOTA_EXCEEDED, Box::new(err))
                        .with_detail(detail)
                }
                err @ UserSearchFilterError::SearchFilterFeatureForbidden(_) => {
                    let detail = err.to_string();
                    ApiError::unprocessable_entity(SEARCH_FILTER_RESTRICTED_FEATURE, Box::new(err))
                        .with_detail(detail)
                }
                UserSearchFilterError::UserServiceError(_) => {
                    ApiError::internal_server_error(INTERNAL_SERVER_ERROR, Box::new(err))
                }
                UserSearchFilterError::PeriodicHybridMatchWriteIncomplete(_) => {
                    ApiError::internal_server_error(INTERNAL_SERVER_ERROR, Box::new(err))
                }
            }
        }
    }
}

#[async_trait::async_trait]
#[mockall::automock]
pub trait UserSearchFilterService {
    async fn find_user_search_filters(
        &self,
        user_id: &UserId,
        sort_by_created: &Option<SortOrder>,
    ) -> Result<Vec<UserSearchFilter>, UserSearchFilterError>;

    async fn find_user_search_filter(
        &self,
        user_id: &UserId,
        user_search_filter_id: &UserSearchFilterId,
    ) -> Result<UserSearchFilter, UserSearchFilterError>;

    async fn create_user_search_filter(
        &self,
        ctx: &RequestContext,
        user_id: &UserId,
        name: UserSearchFilterName,
        search_filter: ProductSearch,
    ) -> Result<UserSearchFilter, UserSearchFilterError>;

    async fn delete_user_search_filter(
        &self,
        ctx: &RequestContext,
        user_id: &UserId,
        user_search_filter_id: &UserSearchFilterId,
    ) -> Result<(), UserSearchFilterError>;

    async fn update_user_search_filter(
        &self,
        ctx: &RequestContext,
        user_id: &UserId,
        user_search_filter_id: &UserSearchFilterId,
        update: UserSearchFilterUpdate,
    ) -> Result<UserSearchFilter, UserSearchFilterError>;

    async fn match_user_search_filters(
        &self,
        product_document: &product::opensearch::product_document::ProductDocument,
    ) -> Result<Vec<UserSearchFilter>, UserSearchFilterError>;

    async fn search_user_search_filters(
        &self,
        search: &UserSearchFilterSearch,
        cursor: &Option<Cursor<serde_json::Value>>,
    ) -> Result<CursoredResult<UserSearchFilter, serde_json::Value>, UserSearchFilterError>;

    async fn find_search_filter_product_match(
        &self,
        user_id: &UserId,
        search_filter_id: &UserSearchFilterId,
        shop_id: &common::shop_id::ShopId,
        shops_product_id: &common::shops_product_id::ShopsProductId,
    ) -> Result<Option<SearchFilterProductMatch>, UserSearchFilterError>;

    async fn find_search_filter_product_matches(
        &self,
        user_id: &UserId,
    ) -> Result<Vec<SearchFilterProductMatch>, UserSearchFilterError>;

    async fn find_search_filter_product_matches_for_filter(
        &self,
        user_id: &UserId,
        search_filter_id: &UserSearchFilterId,
        sort: &Option<Sort<SortSearchFilterMatchField>>,
        cursor: Option<Cursor<OffsetDateTime>>,
    ) -> Result<Vec<SearchFilterProductMatch>, UserSearchFilterError>;

    async fn view_search_filter_matches(
        &self,
        user_id: &UserId,
        search_filter_id: &UserSearchFilterId,
        sort: &Option<Sort<SortSearchFilterMatchField>>,
        cursor: Option<Cursor<OffsetDateTime>>,
    ) -> Result<CursoredResult<SearchFilterProductMatch, OffsetDateTime>, UserSearchFilterError>;

    async fn create_search_filter_product_match(
        &self,
        ctx: &RequestContext,
        product_match: SearchFilterProductMatch,
    ) -> Result<SearchFilterProductMatch, UserSearchFilterError>;

    async fn update_search_filter_product_match(
        &self,
        ctx: &RequestContext,
        user_id: UserId,
        search_filter_id: UserSearchFilterId,
        shop_id: ShopId,
        shops_product_id: ShopsProductId,
        update: UpdateUserSearchFilterMatchCommand,
    ) -> Result<SearchFilterProductMatch, UserSearchFilterError>;

    async fn create_search_filter_product_matches(
        &self,
        ctx: &RequestContext,
        product_matches: Vec<SearchFilterProductMatch>,
    ) -> Result<CreateSearchFilterProductMatchesResult, UserSearchFilterError>;

    async fn count_user_search_filter_matches_for_this_month(
        &self,
        user_id: &UserId,
    ) -> Result<u64, UserSearchFilterError>;
}

#[derive(Debug)]
pub struct CreateSearchFilterProductMatchesResult {
    pub processed: Vec<SearchFilterProductMatch>,
    pub unprocessed: Vec<SearchFilterProductMatch>,
}

pub struct UserSearchFilterServiceImpl<'a> {
    repository: &'a (dyn UserSearchFilterDynamoDbRepository + Sync),
    user_service: &'a (dyn UserService + Sync),
    embedding_service: Option<&'a dyn EmbeddingGenerator>,
    #[cfg(feature = "opensearch")]
    opensearch_repository: Option<
        &'a (dyn crate::opensearch::repository::UserSearchFilterOpenSearchRepository + Sync),
    >,
}

impl<'a> UserSearchFilterServiceImpl<'a> {
    pub fn new(
        repository: &'a (dyn UserSearchFilterDynamoDbRepository + Sync),
        user_service: &'a (dyn UserService + Sync),
    ) -> Self {
        Self {
            repository,
            user_service,
            embedding_service: None,
            #[cfg(feature = "opensearch")]
            opensearch_repository: None,
        }
    }

    pub fn with_embedding_service(
        repository: &'a (dyn UserSearchFilterDynamoDbRepository + Sync),
        user_service: &'a (dyn UserService + Sync),
        embedding_service: &'a dyn EmbeddingGenerator,
    ) -> Self {
        Self {
            repository,
            user_service,
            embedding_service: Some(embedding_service),
            #[cfg(feature = "opensearch")]
            opensearch_repository: None,
        }
    }

    #[cfg(feature = "opensearch")]
    pub fn with_opensearch(
        repository: &'a (dyn UserSearchFilterDynamoDbRepository + Sync),
        user_service: &'a (dyn UserService + Sync),
        opensearch_repository: &'a (
                dyn crate::opensearch::repository::UserSearchFilterOpenSearchRepository + Sync
            ),
    ) -> Self {
        Self {
            repository,
            user_service,
            embedding_service: None,
            opensearch_repository: Some(opensearch_repository),
        }
    }

    #[cfg(feature = "opensearch")]
    pub fn with_opensearch_and_embedding_service(
        repository: &'a (dyn UserSearchFilterDynamoDbRepository + Sync),
        user_service: &'a (dyn UserService + Sync),
        opensearch_repository: &'a (
                dyn crate::opensearch::repository::UserSearchFilterOpenSearchRepository + Sync
            ),
        embedding_service: &'a dyn EmbeddingGenerator,
    ) -> Self {
        Self {
            repository,
            user_service,
            embedding_service: Some(embedding_service),
            opensearch_repository: Some(opensearch_repository),
        }
    }

    async fn count_active_user_search_filter_records(
        &self,
        user_id: &UserId,
    ) -> Result<usize, UserSearchFilterError> {
        Ok(self
            .repository
            .query_user_search_filter_records(user_id, true)
            .await?
            .into_iter()
            .filter(|record| record.state.is_active())
            .count())
    }

    async fn compute_embedding(
        &self,
        search: &ProductSearch,
    ) -> Result<Option<Vec<f32>>, UserSearchFilterError> {
        let Some(embedding_text) = Product::embedding_text(search) else {
            return Ok(None);
        };
        let Some(embedding_service) = self.embedding_service else {
            return Ok(None);
        };

        let embedding_text = EmbeddingText::new(embedding_text)?;
        let embedding = embedding_service
            .embed_search_query(&embedding_text)
            .await?;
        Ok(Some(embedding.into_values()))
    }
}

#[async_trait::async_trait]
impl<'a> UserSearchFilterService for UserSearchFilterServiceImpl<'a> {
    async fn find_user_search_filters(
        &self,
        user_id: &UserId,
        sort_by_created: &Option<SortOrder>,
    ) -> Result<Vec<UserSearchFilter>, UserSearchFilterError> {
        let search_filters = self
            .repository
            .query_user_search_filter_records(
                user_id,
                matches!(sort_by_created.unwrap_or(SortOrder::Asc), SortOrder::Asc),
            )
            .await?
            .into_iter()
            .map(UserSearchFilter::from)
            .collect();
        Ok(search_filters)
    }

    async fn find_user_search_filter(
        &self,
        user_id: &UserId,
        user_search_filter_id: &UserSearchFilterId,
    ) -> Result<UserSearchFilter, UserSearchFilterError> {
        let search_filter = self
            .repository
            .get_user_search_filter_record(user_id, user_search_filter_id)
            .await?
            .map(UserSearchFilter::from)
            .ok_or_else(|| {
                UserSearchFilterError::UserSearchFilterNotFound(*user_id, *user_search_filter_id)
            })?;
        Ok(search_filter)
    }

    async fn create_user_search_filter(
        &self,
        ctx: &RequestContext,
        user_id: &UserId,
        name: UserSearchFilterName,
        search: ProductSearch,
    ) -> Result<UserSearchFilter, UserSearchFilterError> {
        let user: User = self
            .user_service
            .find_user(user_id)
            .await
            .map_err(|e| match e {
                UserServiceError::UserNotFound(id) => UserSearchFilterError::UserNotFound(id),
                other => UserSearchFilterError::UserServiceError(other),
            })?;

        let limit = user.tier.search_filter_quota();
        let filter_count = self
            .count_active_user_search_filter_records(user_id)
            .await?;
        if filter_count as u32 >= limit {
            return Err(UserSearchFilterError::SearchFilterQuotaExceeded(
                filter_count as u32,
                limit,
            ));
        }

        let () = user
            .tier
            .check_search_filter_features(&search)
            .map_err(UserSearchFilterError::SearchFilterFeatureForbidden)?;

        let now = OffsetDateTime::now_utc();
        let user_search_filter = UserSearchFilter {
            user_id: *user_id,
            user_search_filter_id: UserSearchFilterId::new(),
            name,
            notifications: true,
            state: ResourceState::Active,
            created_by: ctx.actor,
            updated_by: ctx.actor,
            created: now,
            updated: now,
            last_hybrid_search_matched: now,
            embedding: self.compute_embedding(&search).await?,
            search,
        };

        let _ = self
            .repository
            .put_user_search_filter_record(user_search_filter.clone().into())
            .await?;

        info!(actor = %ctx.actor, userId = %user_id, userSearchFilterId = %user_search_filter.user_search_filter_id, "Created UserSearchFilter.");

        Ok(user_search_filter)
    }

    async fn delete_user_search_filter(
        &self,
        ctx: &RequestContext,
        user_id: &UserId,
        user_search_filter_id: &UserSearchFilterId,
    ) -> Result<(), UserSearchFilterError> {
        // exists guard
        let _ = self
            .find_user_search_filter(user_id, user_search_filter_id)
            .await?;
        let _ = self
            .repository
            .delete_user_search_filter_record(user_id, user_search_filter_id)
            .await?;
        info!(actor = %ctx.actor, userId = %user_id, userSearchFilterId = %user_search_filter_id, "Deleted UserSearchFilter.");
        Ok(())
    }

    async fn update_user_search_filter(
        &self,
        ctx: &RequestContext,
        user_id: &UserId,
        user_search_filter_id: &UserSearchFilterId,
        update: UserSearchFilterUpdate,
    ) -> Result<UserSearchFilter, UserSearchFilterError> {
        let user: User = self
            .user_service
            .find_user(user_id)
            .await
            .map_err(|e| match e {
                UserServiceError::UserNotFound(id) => UserSearchFilterError::UserNotFound(id),
                other => UserSearchFilterError::UserServiceError(other),
            })?;
        let () = user
            .tier
            .check_search_filter_update_features(&update)
            .map_err(UserSearchFilterError::SearchFilterFeatureForbidden)?;

        // exists guard
        let existing = self
            .find_user_search_filter(user_id, user_search_filter_id)
            .await?;

        if matches!(update.state, Some(ResourceState::Active)) && !existing.state.is_active() {
            user.tier
                .check_search_filter_features(&existing.search)
                .map_err(UserSearchFilterError::SearchFilterFeatureForbidden)?;
            let limit = user.tier.search_filter_quota();
            let filter_count = self
                .count_active_user_search_filter_records(user_id)
                .await?;
            if filter_count as u32 >= limit {
                return Err(UserSearchFilterError::SearchFilterQuotaExceeded(
                    filter_count as u32,
                    limit,
                ));
            }
        }

        let embedding_update =
            if update.product_query.is_some() || update.enhanced_search_description.is_some() {
                let mut updated_search = existing.search.clone();
                if let Some(product_query) = &update.product_query {
                    updated_search.product_query = product_query.clone();
                }
                if let Some(enhanced_search_description) = &update.enhanced_search_description {
                    updated_search.enhanced_search_description =
                        Some(enhanced_search_description.clone());
                }

                if Product::embedding_text(&updated_search).is_some() {
                    self.compute_embedding(&updated_search).await?
                } else {
                    Some(Vec::new())
                }
            } else {
                None
            };

        let mut record_update: crate::dynamodb::user_search_filter_record_update::UserSearchFilterRecordUpdate =
            (update, ctx.actor).into();
        record_update.updated_by = ctx.actor.into();
        record_update.embedding = embedding_update;

        let updated_opt = self
            .repository
            .update_user_search_filter_record(user_id, user_search_filter_id, record_update)
            .await?;
        info!(actor = %ctx.actor, userId = %user_id, userSearchFilterId = %user_search_filter_id, "Updated UserSearchFilter.");
        match updated_opt {
            Some(updated) => Ok(updated.into()),
            None => {
                self.find_user_search_filter(user_id, user_search_filter_id)
                    .await
            }
        }
    }

    async fn match_user_search_filters(
        &self,
        product_document: &ProductDocument,
    ) -> Result<Vec<UserSearchFilter>, UserSearchFilterError> {
        #[cfg(feature = "opensearch")]
        {
            use serde::ser::Error as _;

            let opensearch_repo = self.opensearch_repository.ok_or_else(|| {
                UserSearchFilterError::OpenSearchError(opensearch::Error::from(
                    serde_json::Error::custom(
                        "OpenSearch repository not configured. Use UserSearchFilterServiceImpl::with_opensearch() to construct the service.",
                    ),
                ))
            })?;
            let matched_documents = opensearch_repo
                .percolate(product_document)
                .await?
                .into_iter()
                .map(UserSearchFilter::from)
                .collect();
            Ok(matched_documents)
        }
        #[cfg(not(feature = "opensearch"))]
        {
            let _ = product_document;
            unimplemented!("match_user_search_filters requires the 'opensearch' feature")
        }
    }

    async fn search_user_search_filters(
        &self,
        search: &UserSearchFilterSearch,
        cursor: &Option<Cursor<serde_json::Value>>,
    ) -> Result<CursoredResult<UserSearchFilter, serde_json::Value>, UserSearchFilterError> {
        #[cfg(feature = "opensearch")]
        {
            use serde::ser::Error as _;

            let opensearch_repo = self.opensearch_repository.ok_or_else(|| {
                UserSearchFilterError::OpenSearchError(opensearch::Error::from(
                    serde_json::Error::custom(
                        "OpenSearch repository not configured. Use UserSearchFilterServiceImpl::with_opensearch() to construct the service.",
                    ),
                ))
            })?;
            let search_response = opensearch_repo.query_documents(search, cursor).await?;
            let cursor = Cursor {
                size: search_response.hits.hits.len() as u64,
                search_after: search_response
                    .hits
                    .hits
                    .last()
                    .and_then(|last| last.sort.clone()),
            };
            let items = search_response
                .hits
                .hits
                .into_iter()
                .map(|hit| hit.source.into())
                .collect();
            Ok(CursoredResult {
                items,
                cursor,
                total: Some(search_response.hits.total.value),
            })
        }
        #[cfg(not(feature = "opensearch"))]
        {
            let _ = search;
            let _ = cursor;
            unimplemented!("search_user_search_filters requires the 'opensearch' feature")
        }
    }

    async fn find_search_filter_product_match(
        &self,
        user_id: &UserId,
        search_filter_id: &UserSearchFilterId,
        shop_id: &common::shop_id::ShopId,
        shops_product_id: &common::shops_product_id::ShopsProductId,
    ) -> Result<Option<SearchFilterProductMatch>, UserSearchFilterError> {
        let record = self
            .repository
            .get_user_search_filter_match_record(
                user_id,
                search_filter_id,
                shop_id,
                shops_product_id,
            )
            .await?;
        Ok(record.map(SearchFilterProductMatch::from))
    }

    async fn find_search_filter_product_matches(
        &self,
        user_id: &UserId,
    ) -> Result<Vec<SearchFilterProductMatch>, UserSearchFilterError> {
        let records = self
            .repository
            .query_user_search_filter_match_records_all(user_id)
            .await?;
        Ok(records
            .into_iter()
            .map(SearchFilterProductMatch::from)
            .collect())
    }

    async fn find_search_filter_product_matches_for_filter(
        &self,
        user_id: &UserId,
        search_filter_id: &UserSearchFilterId,
        sort: &Option<Sort<SortSearchFilterMatchField>>,
        cursor: Option<Cursor<OffsetDateTime>>,
    ) -> Result<Vec<SearchFilterProductMatch>, UserSearchFilterError> {
        let sort = sort.unwrap_or(Sort {
            sort: SortSearchFilterMatchField::Created,
            order: SortOrder::Asc,
        });
        let scan_index_forward = matches!(sort.order, SortOrder::Asc);
        let records = self
            .repository
            .query_user_search_filter_match_records_for_filter(
                user_id,
                search_filter_id,
                cursor,
                scan_index_forward,
            )
            .await?;
        Ok(records
            .into_iter()
            .map(SearchFilterProductMatch::from)
            .collect())
    }

    async fn view_search_filter_matches(
        &self,
        user_id: &UserId,
        search_filter_id: &UserSearchFilterId,
        sort: &Option<Sort<SortSearchFilterMatchField>>,
        cursor: Option<Cursor<OffsetDateTime>>,
    ) -> Result<CursoredResult<SearchFilterProductMatch, OffsetDateTime>, UserSearchFilterError>
    {
        let sort = sort.unwrap_or(Sort {
            sort: SortSearchFilterMatchField::Created,
            order: SortOrder::Asc,
        });
        let scan_index_forward = matches!(sort.order, SortOrder::Asc);
        let cursor = cursor.unwrap_or_default();
        let paged_records = self
            .repository
            .query_user_search_filter_match_records_for_filter(
                user_id,
                search_filter_id,
                Some(cursor),
                scan_index_forward,
            )
            .await?;
        let last = paged_records.last().cloned();
        let matches: Vec<SearchFilterProductMatch> = paged_records
            .into_iter()
            .map(SearchFilterProductMatch::from)
            .collect();

        let total = self
            .repository
            .count_user_search_filter_match_records_for_filter(
                user_id,
                search_filter_id,
                Some(cursor),
                scan_index_forward,
            )
            .await?;

        Ok(CursoredResult {
            cursor: Cursor {
                size: matches.len() as u64,
                search_after: last.map(|last| last.created),
            },
            items: matches,
            total: Some(total),
        })
    }

    async fn create_search_filter_product_match(
        &self,
        ctx: &RequestContext,
        product_match: SearchFilterProductMatch,
    ) -> Result<SearchFilterProductMatch, UserSearchFilterError> {
        let record = UserSearchFilterMatchRecord::from(product_match.clone());
        self.repository
            .put_user_search_filter_match_record(record)
            .await?;
        info!(
            actor = %ctx.actor,
            userId = %product_match.user_id,
            searchFilterId = %product_match.user_search_filter_id,
            shopId = %product_match.shop_id,
            shopsProductId = %product_match.shops_product_id,
            "Created SearchFilterProductMatch."
        );
        Ok(product_match)
    }

    async fn update_search_filter_product_match(
        &self,
        ctx: &RequestContext,
        user_id: UserId,
        search_filter_id: UserSearchFilterId,
        shop_id: ShopId,
        shops_product_id: ShopsProductId,
        update: UpdateUserSearchFilterMatchCommand,
    ) -> Result<SearchFilterProductMatch, UserSearchFilterError> {
        let existing = self
            .repository
            .get_user_search_filter_match_record(
                &user_id,
                &search_filter_id,
                &shop_id,
                &shops_product_id,
            )
            .await?
            .ok_or_else(|| {
                UserSearchFilterError::UserSearchFilterMatchNotFound(
                    user_id,
                    search_filter_id,
                    shop_id,
                    shops_product_id.clone(),
                )
            })?;

        if update.is_empty() {
            return Ok(existing.into());
        }

        let updated = self
            .repository
            .update_user_search_filter_match_record(
                &user_id,
                &search_filter_id,
                &shop_id,
                &shops_product_id,
                UserSearchFilterMatchRecordUpdate::from((update, ctx.actor)),
            )
            .await?
            .ok_or_else(|| {
                UserSearchFilterError::SdkUpdateItemError(SdkError::construction_failure(
                    "Failed parsing DynamoDB UpdateItem Response-Payload",
                ))
            })?;

        info!(actor = %ctx.actor, userId = %user_id, searchFilterId = %search_filter_id, shopId = %shop_id, shopsProductId = %shops_product_id, "Updated SearchFilterProductMatch.");

        Ok(updated.into())
    }

    async fn create_search_filter_product_matches(
        &self,
        ctx: &RequestContext,
        product_matches: Vec<SearchFilterProductMatch>,
    ) -> Result<CreateSearchFilterProductMatchesResult, UserSearchFilterError> {
        if product_matches.is_empty() {
            return Ok(CreateSearchFilterProductMatchesResult {
                processed: vec![],
                unprocessed: vec![],
            });
        }

        // Pair each match with its record so ordering is guaranteed when chunking.
        let pairs: Vec<(SearchFilterProductMatch, UserSearchFilterMatchRecord)> = product_matches
            .into_iter()
            .map(|m| {
                let record = UserSearchFilterMatchRecord::from(m.clone());
                (m, record)
            })
            .collect();

        let mut processed = Vec::new();
        let mut unprocessed = Vec::new();

        for chunk in pairs.chunks(25) {
            let (batch_matches, batch_records): (Vec<_>, Vec<_>) = chunk.iter().cloned().unzip();
            let batch = Batch::<UserSearchFilterMatchRecord, 25>::try_from(batch_records)
                .expect("chunk size is at most 25");

            match self
                .repository
                .put_user_search_filter_match_records(batch)
                .await
            {
                Ok(output) => {
                    let failed_keys = extract_failed_sort_keys(output);

                    for m in batch_matches {
                        let sk = crate::dynamodb::user_search_filter_match_record::mk_sk(
                            &m.user_search_filter_id,
                            &m.shop_id,
                            &m.shops_product_id,
                        );
                        if failed_keys.contains(&sk) {
                            unprocessed.push(m);
                        } else {
                            processed.push(m);
                        }
                    }
                }
                Err(err) => {
                    warn!(
                        actor = %ctx.actor,
                        error = ?err,
                        "Failed writing UserSearchFilterMatchRecord batch."
                    );
                    unprocessed.extend(batch_matches);
                }
            }
        }

        Ok(CreateSearchFilterProductMatchesResult {
            processed,
            unprocessed,
        })
    }

    async fn count_user_search_filter_matches_for_this_month(
        &self,
        user_id: &UserId,
    ) -> Result<u64, UserSearchFilterError> {
        let now = OffsetDateTime::now_utc();
        let from = now
            .replace_day(1)
            .expect("day 1 is always valid")
            .replace_hour(0)
            .expect("hour 0 is always valid")
            .replace_minute(0)
            .expect("minute 0 is always valid")
            .replace_second(0)
            .expect("second 0 is always valid")
            .replace_nanosecond(0)
            .expect("nanosecond 0 is always valid");
        let to = now;
        let count = self
            .repository
            .count_user_search_filter_match_records_for_between(user_id, &from, &to)
            .await?;
        Ok(count)
    }
}

fn extract_failed_sort_keys(
    output: aws_sdk_dynamodb::operation::batch_write_item::BatchWriteItemOutput,
) -> std::collections::HashSet<String> {
    output
        .unprocessed_items
        .unwrap_or_default()
        .into_values()
        .flatten()
        .filter_map(|req| req.put_request)
        .filter_map(|put| {
            match serde_dynamo::from_item::<_, UserSearchFilterMatchRecord>(put.item) {
                Ok(record) => Some(record.sk),
                Err(err) => {
                    error!(
                        error = ?err,
                        r#type = std::any::type_name::<UserSearchFilterMatchRecord>(),
                        "Failed parsing unprocessed item from BatchWriteItem output."
                    );
                    None
                }
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use embedding::{
        EmbeddingError, EmbeddingGenerator, EmbeddingImageUrl, EmbeddingText, EmbeddingVector,
    };

    struct FixedEmbeddingGenerator(Vec<f32>);

    #[async_trait::async_trait]
    impl EmbeddingGenerator for FixedEmbeddingGenerator {
        async fn embed_product(
            &self,
            _: &EmbeddingText,
            _: Option<&EmbeddingText>,
            _: Option<&EmbeddingImageUrl>,
        ) -> Result<EmbeddingVector, EmbeddingError> {
            Err(EmbeddingError::InvalidInput {
                reason: "test generator supports queries only",
            })
        }

        async fn embed_search_query(
            &self,
            _: &EmbeddingText,
        ) -> Result<EmbeddingVector, EmbeddingError> {
            EmbeddingVector::try_new(self.0.clone())
        }
    }

    fn system_ctx() -> common::actor::RequestContext {
        common::actor::RequestContext {
            actor: common::actor::domain::Actor::System,
        }
    }

    mod find_search_filters {
        use crate::dynamodb::repository::MockUserSearchFilterDynamoDbRepository;
        use crate::dynamodb::user_search_filter_record::UserSearchFilterRecord;
        use crate::service::user_search_filter_service::{
            UserSearchFilterError, UserSearchFilterService, UserSearchFilterServiceImpl,
        };
        use aws_sdk_dynamodb::{
            config::http::HttpResponse,
            error::{ConnectorError, SdkError},
        };
        use common::sort::SortOrder;
        use common::user_id::UserId;

        #[rstest::rstest]
        #[case::empty(0)]
        #[case::non_empty(42)]
        #[tokio::test]
        #[trace]
        async fn should_return_search_filters(#[case] count: usize) {
            let mut repository = MockUserSearchFilterDynamoDbRepository::default();
            repository
                .expect_query_user_search_filter_records()
                .return_once(move |_, _| {
                    Box::pin(async move { Ok(fake::vec![UserSearchFilterRecord; count]) })
                });
            let user_service = user::service::user_service::MockUserService::default();
            let service = UserSearchFilterServiceImpl::new(&repository, &user_service);
            let actual = service
                .find_user_search_filters(&UserId::new(), &Some(SortOrder::Asc))
                .await;
            assert!(actual.is_ok());
        }

        #[tokio::test]
        #[rstest::rstest]
        #[case::construction_failure(SdkError::construction_failure("Something went wrong"))]
        #[case::timeout(SdkError::timeout_error("Something went wrong"))]
        #[case::dispatch_failure(SdkError::dispatch_failure(ConnectorError::user("Something went wrong".into())))]
        #[case::response_error(SdkError::response_error(
            "Something went wrong",
            HttpResponse::new(500u16.try_into().unwrap(), "{}".into())
        ))]
        #[case::service_error(SdkError::service_error(
            aws_sdk_dynamodb::operation::query::QueryError::unhandled("Something went wrong"),
            HttpResponse::new(500u16.try_into().unwrap(), "{}".into())
        ))]
        #[trace]
        async fn should_propagate_sdk_error(
            #[case] expected: SdkError<
                aws_sdk_dynamodb::operation::query::QueryError,
                aws_sdk_dynamodb::config::http::HttpResponse,
            >,
        ) {
            let mut repository = MockUserSearchFilterDynamoDbRepository::default();
            repository
                .expect_query_user_search_filter_records()
                .return_once(|_, _| Box::pin(async { Err(expected) }));
            let user_service = user::service::user_service::MockUserService::default();
            let service = UserSearchFilterServiceImpl::new(&repository, &user_service);
            let actual = service
                .find_user_search_filters(&UserId::new(), &Some(SortOrder::Desc))
                .await;

            assert!(actual.is_err());
            match actual.unwrap_err() {
                UserSearchFilterError::SdkQueryError(_) => {}
                _ => panic!("expected SearchFilterError::SdkQueryError"),
            }
        }
    }

    mod find_search_filter {
        use crate::dynamodb::repository::MockUserSearchFilterDynamoDbRepository;
        use crate::service::user_search_filter_service::{
            UserSearchFilterError, UserSearchFilterService, UserSearchFilterServiceImpl,
        };
        use aws_sdk_dynamodb::{
            config::http::HttpResponse,
            error::{ConnectorError, SdkError},
        };
        use common::user_id::UserId;
        use common::user_search_filter_id::UserSearchFilterId;
        use fake::{Fake, Faker};

        #[tokio::test]
        async fn should_return_search_filter_when_exists() {
            let mut repository = MockUserSearchFilterDynamoDbRepository::default();
            repository
                .expect_get_user_search_filter_record()
                .return_once(|_, _| Box::pin(async { Ok(Some(Faker.fake())) }));
            let user_service = user::service::user_service::MockUserService::default();
            let service = UserSearchFilterServiceImpl::new(&repository, &user_service);
            let actual = service
                .find_user_search_filter(&UserId::new(), &UserSearchFilterId::new())
                .await;
            assert!(actual.is_ok());
        }

        #[tokio::test]
        async fn should_return_search_filter_not_found_error_when_not_exists() {
            let mut repository = MockUserSearchFilterDynamoDbRepository::default();
            repository
                .expect_get_user_search_filter_record()
                .return_once(|_, _| Box::pin(async { Ok(None) }));
            let user_service = user::service::user_service::MockUserService::default();
            let service = UserSearchFilterServiceImpl::new(&repository, &user_service);
            let user_id = UserId::new();
            let user_search_filter_id = UserSearchFilterId::new();
            let actual = service
                .find_user_search_filter(&user_id, &user_search_filter_id)
                .await;

            assert!(actual.is_err());
            match actual.unwrap_err() {
                UserSearchFilterError::UserSearchFilterNotFound(
                    err_user_id,
                    err_user_search_filter_id,
                ) => {
                    assert_eq!(err_user_id, user_id);
                    assert_eq!(err_user_search_filter_id, user_search_filter_id);
                }
                _ => panic!("expected SearchFilterError::SearchFilterNotFound"),
            }
        }

        #[tokio::test]
        #[rstest::rstest]
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
        #[trace]
        async fn should_propagate_sdk_error(
            #[case] expected: SdkError<
                aws_sdk_dynamodb::operation::get_item::GetItemError,
                aws_sdk_dynamodb::config::http::HttpResponse,
            >,
        ) {
            let mut repository = MockUserSearchFilterDynamoDbRepository::default();
            repository
                .expect_get_user_search_filter_record()
                .return_once(|_, _| Box::pin(async { Err(expected) }));
            let user_service = user::service::user_service::MockUserService::default();
            let service = UserSearchFilterServiceImpl::new(&repository, &user_service);
            let actual = service
                .find_user_search_filter(&UserId::new(), &UserSearchFilterId::new())
                .await;

            assert!(actual.is_err());
            match actual.unwrap_err() {
                UserSearchFilterError::SdkGetItemError(_) => {}
                _ => panic!("expected SearchFilterError::SdkGetItemError"),
            }
        }
    }

    mod create_search_filter {
        use super::FixedEmbeddingGenerator;
        use crate::core::quota::SearchFilterQuota;
        use crate::dynamodb::repository::MockUserSearchFilterDynamoDbRepository;
        use crate::service::user_search_filter_service::{
            UserSearchFilterError, UserSearchFilterService, UserSearchFilterServiceImpl,
        };
        use aws_sdk_dynamodb::{
            config::http::HttpResponse,
            error::{ConnectorError, SdkError},
            operation::put_item::PutItemOutput,
        };
        use common::user_id::UserId;
        use fake::{Fake, Faker};
        use product::core::product_search::{ProductSearch, ProductSearchSerdeField};
        use user::core::tier::UserTier;
        use user::core::user::User;
        use user::service::user_service::{MockUserService, UserServiceError};

        #[tokio::test]
        async fn should_create_search_filter() {
            let mut repository = MockUserSearchFilterDynamoDbRepository::default();
            let mut user_service = MockUserService::default();

            user_service
                .expect_find_user()
                .return_once(|_| Box::pin(async { Ok(fake::Fake::fake(&fake::Faker)) }));

            repository
                .expect_query_user_search_filter_records()
                .return_once(|_, _| Box::pin(async { Ok(vec![]) }));

            repository
                .expect_put_user_search_filter_record()
                .return_once(|_| Box::pin(async { Ok(PutItemOutput::builder().build()) }));

            let service = UserSearchFilterServiceImpl::new(&repository, &user_service);
            let actual = service
                .create_user_search_filter(
                    &super::system_ctx(),
                    &UserId::new(),
                    Faker.fake(),
                    Faker.fake(),
                )
                .await;
            assert!(actual.is_ok());
        }

        #[tokio::test]
        async fn should_compute_embedding_when_creating_search_filter_with_query_text() {
            let mut repository = MockUserSearchFilterDynamoDbRepository::default();
            let mut user_service = MockUserService::default();
            let embedding_service = FixedEmbeddingGenerator(vec![1.0; 768]);

            user_service.expect_find_user().return_once(|_| {
                Box::pin(async {
                    let mut user: User = fake::Fake::fake(&fake::Faker);
                    user.tier = UserTier::Ultimate;
                    Ok(user)
                })
            });
            repository
                .expect_query_user_search_filter_records()
                .return_once(|_, _| Box::pin(async { Ok(vec![]) }));

            repository
                .expect_put_user_search_filter_record()
                .return_once(|record| {
                    assert_eq!(record.embedding, Some(vec![1.0 / 768_f32.sqrt(); 768]));
                    Box::pin(async { Ok(PutItemOutput::builder().build()) })
                });

            let service = UserSearchFilterServiceImpl::with_embedding_service(
                &repository,
                &user_service,
                &embedding_service,
            );
            let search = ProductSearch::new(
                common::language::domain::Language::En,
                common::currency::domain::Currency::Eur,
            )
            .with_product_query("antique lamp".try_into().unwrap());
            let actual = service
                .create_user_search_filter(
                    &super::system_ctx(),
                    &UserId::new(),
                    Faker.fake(),
                    search,
                )
                .await
                .unwrap();

            assert_eq!(actual.embedding, Some(vec![1.0 / 768_f32.sqrt(); 768]));
        }

        #[tokio::test]
        #[rstest::rstest]
        #[case::construction_failure(SdkError::construction_failure("Something went wrong"))]
        #[case::timeout(SdkError::timeout_error("Something went wrong"))]
        #[case::dispatch_failure(SdkError::dispatch_failure(ConnectorError::user("Something went wrong".into())))]
        #[case::response_error(SdkError::response_error(
            "Something went wrong",
            HttpResponse::new(500u16.try_into().unwrap(), "{}".into())
        ))]
        #[case::service_error(SdkError::service_error(
            aws_sdk_dynamodb::operation::put_item::PutItemError::unhandled("Something went wrong"),
            HttpResponse::new(500u16.try_into().unwrap(), "{}".into())
        ))]
        #[trace]
        async fn should_propagate_sdk_error(
            #[case] expected: SdkError<
                aws_sdk_dynamodb::operation::put_item::PutItemError,
                aws_sdk_dynamodb::config::http::HttpResponse,
            >,
        ) {
            let mut repository = MockUserSearchFilterDynamoDbRepository::default();
            let mut user_service = MockUserService::default();

            user_service
                .expect_find_user()
                .return_once(|_| Box::pin(async { Ok(fake::Fake::fake(&fake::Faker)) }));

            repository
                .expect_query_user_search_filter_records()
                .return_once(|_, _| Box::pin(async { Ok(vec![]) }));

            repository
                .expect_put_user_search_filter_record()
                .return_once(|_| Box::pin(async { Err(expected) }));

            let service = UserSearchFilterServiceImpl::new(&repository, &user_service);
            let actual = service
                .create_user_search_filter(
                    &super::system_ctx(),
                    &UserId::new(),
                    Faker.fake(),
                    Faker.fake(),
                )
                .await;

            assert!(actual.is_err());
            match actual.unwrap_err() {
                UserSearchFilterError::SdkPutItemError(_) => {}
                _ => panic!("expected SearchFilterError::SdkPutItemError"),
            }
        }

        #[tokio::test]
        async fn should_err_search_filter_quota_exceeded_when_limit_reached() {
            use crate::dynamodb::user_search_filter_record::UserSearchFilterRecord;

            let mut repository = MockUserSearchFilterDynamoDbRepository::default();
            let mut user_service = MockUserService::default();

            user_service.expect_find_user().return_once(|_| {
                Box::pin(async {
                    let mut user: User = fake::Fake::fake(&fake::Faker);
                    user.tier = user::core::tier::UserTier::Free;
                    Ok(user)
                })
            });

            let limit = user::core::tier::UserTier::Free.search_filter_quota();
            repository
                .expect_query_user_search_filter_records()
                .return_once(move |_, _| {
                    Box::pin(async move { Ok(fake::vec![UserSearchFilterRecord; limit as usize]) })
                });

            let service = UserSearchFilterServiceImpl::new(&repository, &user_service);
            let actual = service
                .create_user_search_filter(
                    &super::system_ctx(),
                    &UserId::new(),
                    Faker.fake(),
                    Faker.fake(),
                )
                .await
                .unwrap_err();

            match actual {
                UserSearchFilterError::SearchFilterQuotaExceeded(actual_count, actual_limit) => {
                    assert_eq!(limit, actual_count);
                    assert_eq!(limit, actual_limit);
                }
                err => panic!(
                    "Expected 'UserSearchFilterError::SearchFilterQuotaExceeded' but got '{err}'"
                ),
            }
        }

        #[tokio::test]
        async fn should_err_user_not_found_when_user_not_exists() {
            let user_id = UserId::new();
            let mut user_service = MockUserService::default();
            user_service.expect_find_user().return_once(move |_| {
                Box::pin(async move { Err(UserServiceError::UserNotFound(user_id)) })
            });

            let repository = MockUserSearchFilterDynamoDbRepository::default();
            let service = UserSearchFilterServiceImpl::new(&repository, &user_service);
            let actual = service
                .create_user_search_filter(
                    &super::system_ctx(),
                    &user_id,
                    Faker.fake(),
                    Faker.fake(),
                )
                .await
                .unwrap_err();

            match actual {
                UserSearchFilterError::UserNotFound(err_user_id) => {
                    assert_eq!(user_id, err_user_id);
                }
                err => panic!("Expected 'UserSearchFilterError::UserNotFound' but got '{err}'"),
            }
        }

        #[tokio::test]
        async fn should_err_search_filter_feature_forbidden_when_free_tier_creates_with_forbidden_features()
         {
            use crate::core::quota::SearchFilterQuota;
            use product::core::product_search::ProductSearch;
            use user::core::user::User;

            let mut repository = MockUserSearchFilterDynamoDbRepository::default();
            let mut user_service = MockUserService::default();

            user_service.expect_find_user().return_once(|_| {
                Box::pin(async {
                    let mut user: User = fake::Fake::fake(&fake::Faker);
                    user.tier = user::core::tier::UserTier::Free;
                    Ok(user)
                })
            });

            repository
                .expect_query_user_search_filter_records()
                .return_once(|_, _| Box::pin(async { Ok(vec![]) }));

            let service = UserSearchFilterServiceImpl::new(&repository, &user_service);

            // Generate searches until we find one with forbidden features for Free tier
            let search: ProductSearch = (0..100)
                .find_map(|_| {
                    let s: ProductSearch = Faker.fake();
                    if user::core::tier::UserTier::Free
                        .check_search_filter_features(&s)
                        .is_err()
                    {
                        Some(s)
                    } else {
                        None
                    }
                })
                .expect("Should generate a search with forbidden features");

            let actual = service
                .create_user_search_filter(
                    &super::system_ctx(),
                    &UserId::new(),
                    Faker.fake(),
                    search,
                )
                .await
                .unwrap_err();

            match actual {
                UserSearchFilterError::SearchFilterFeatureForbidden(_) => {}
                err => panic!(
                    "Expected 'UserSearchFilterError::SearchFilterFeatureForbidden' but got '{err}'"
                ),
            }
        }

        #[tokio::test]
        #[rstest::rstest]
        #[case(user::core::tier::UserTier::Pro)]
        #[case(user::core::tier::UserTier::Ultimate)]
        async fn should_allow_paid_tier_to_use_all_features(
            #[case] tier: user::core::tier::UserTier,
        ) {
            use user::core::user::User;

            let mut repository = MockUserSearchFilterDynamoDbRepository::default();
            let mut user_service = MockUserService::default();

            user_service.expect_find_user().return_once(move |_| {
                Box::pin(async move {
                    let mut user: User = fake::Fake::fake(&fake::Faker);
                    user.tier = tier;
                    Ok(user)
                })
            });

            repository
                .expect_query_user_search_filter_records()
                .return_once(|_, _| Box::pin(async { Ok(vec![]) }));

            repository
                .expect_put_user_search_filter_record()
                .return_once(|_| Box::pin(async { Ok(PutItemOutput::builder().build()) }));

            let service = UserSearchFilterServiceImpl::new(&repository, &user_service);

            let actual = service
                .create_user_search_filter(
                    &super::system_ctx(),
                    &UserId::new(),
                    Faker.fake(),
                    Faker.fake(),
                )
                .await;

            assert!(actual.is_ok());
        }

        #[tokio::test]
        #[rstest::rstest]
        #[case(user::core::tier::UserTier::Pro)]
        #[case(user::core::tier::UserTier::Free)]
        async fn should_err_enhanced_search_description_feature_forbidden_when_non_ultimate_tier_creates_with_enhanced_search_description(
            #[case] tier: user::core::tier::UserTier,
        ) {
            use product::core::product_search::ProductSearch;
            use user::core::user::User;

            let mut repository = MockUserSearchFilterDynamoDbRepository::default();
            let mut user_service = MockUserService::default();

            user_service.expect_find_user().return_once(move |_| {
                Box::pin(async move {
                    let mut user: User = fake::Fake::fake(&fake::Faker);
                    user.tier = tier;
                    Ok(user)
                })
            });

            repository
                .expect_query_user_search_filter_records()
                .return_once(|_, _| Box::pin(async { Ok(vec![]) }));

            let service = UserSearchFilterServiceImpl::new(&repository, &user_service);

            let mut search: ProductSearch = Faker.fake();
            search.enhanced_search_description = Some(Faker.fake());
            let actual = service
                .create_user_search_filter(
                    &super::system_ctx(),
                    &UserId::new(),
                    Faker.fake(),
                    search,
                )
                .await
                .unwrap_err();

            match actual {
                UserSearchFilterError::SearchFilterFeatureForbidden(
                    ProductSearchSerdeField::EnhancedSearchDescription,
                ) => {}
                err => panic!(
                    "Expected 'UserSearchFilterError::SearchFilterFeatureForbidden(ProductSearchSerdeField::EnhancedSearchDescription)' but got '{err}'"
                ),
            }
        }

        #[tokio::test]
        async fn should_allow_ultimate_tier_to_use_enhanced_search_description() {
            use user::core::user::User;

            let mut repository = MockUserSearchFilterDynamoDbRepository::default();
            let mut user_service = MockUserService::default();

            user_service.expect_find_user().return_once(|_| {
                Box::pin(async {
                    let mut user: User = fake::Fake::fake(&fake::Faker);
                    user.tier = user::core::tier::UserTier::Ultimate;
                    Ok(user)
                })
            });

            repository
                .expect_query_user_search_filter_records()
                .return_once(|_, _| Box::pin(async { Ok(vec![]) }));

            repository
                .expect_put_user_search_filter_record()
                .return_once(|_| Box::pin(async { Ok(PutItemOutput::builder().build()) }));

            let service = UserSearchFilterServiceImpl::new(&repository, &user_service);

            let mut search = Faker.fake::<product::core::product_search::ProductSearch>();
            search.enhanced_search_description = Some(Faker.fake());
            let actual = service
                .create_user_search_filter(
                    &super::system_ctx(),
                    &UserId::new(),
                    Faker.fake(),
                    search,
                )
                .await;

            assert!(actual.is_ok());
        }
    }

    mod delete_search_filter {
        use crate::dynamodb::repository::MockUserSearchFilterDynamoDbRepository;
        use crate::service::user_search_filter_service::{
            UserSearchFilterError, UserSearchFilterService, UserSearchFilterServiceImpl,
        };
        use aws_sdk_dynamodb::{
            config::http::HttpResponse,
            error::{ConnectorError, SdkError},
            operation::delete_item::DeleteItemOutput,
        };
        use common::user_id::UserId;
        use common::user_search_filter_id::UserSearchFilterId;
        use fake::{Fake, Faker};

        #[tokio::test]
        async fn should_delete_search_filter_when_exists() {
            let mut repository = MockUserSearchFilterDynamoDbRepository::default();
            repository
                .expect_get_user_search_filter_record()
                .return_once(|_, _| Box::pin(async { Ok(Some(Faker.fake())) }));
            repository
                .expect_delete_user_search_filter_record()
                .return_once(|_, _| Box::pin(async { Ok(DeleteItemOutput::builder().build()) }));
            let user_service = user::service::user_service::MockUserService::default();
            let service = UserSearchFilterServiceImpl::new(&repository, &user_service);
            let actual = service
                .delete_user_search_filter(
                    &super::system_ctx(),
                    &UserId::new(),
                    &UserSearchFilterId::new(),
                )
                .await;
            assert!(actual.is_ok());
        }

        #[tokio::test]
        async fn should_return_search_filter_not_found_error_when_not_exists() {
            let mut repository = MockUserSearchFilterDynamoDbRepository::default();
            repository
                .expect_get_user_search_filter_record()
                .return_once(|_, _| Box::pin(async { Ok(None) }));
            let user_id = UserId::new();
            let user_search_filter_id = UserSearchFilterId::new();
            let user_service = user::service::user_service::MockUserService::default();
            let service = UserSearchFilterServiceImpl::new(&repository, &user_service);
            let actual = service
                .delete_user_search_filter(&super::system_ctx(), &user_id, &user_search_filter_id)
                .await;

            assert!(actual.is_err());
            match actual.unwrap_err() {
                UserSearchFilterError::UserSearchFilterNotFound(
                    err_user_id,
                    err_user_search_filter_id,
                ) => {
                    assert_eq!(err_user_id, user_id);
                    assert_eq!(err_user_search_filter_id, user_search_filter_id);
                }
                _ => panic!("expected SearchFilterError::SearchFilterNotFound"),
            }
        }

        #[tokio::test]
        #[rstest::rstest]
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
        #[trace]
        async fn should_propagate_sdk_error_for_find(
            #[case] expected: SdkError<
                aws_sdk_dynamodb::operation::get_item::GetItemError,
                aws_sdk_dynamodb::config::http::HttpResponse,
            >,
        ) {
            let mut repository = MockUserSearchFilterDynamoDbRepository::default();
            repository
                .expect_get_user_search_filter_record()
                .return_once(|_, _| Box::pin(async { Err(expected) }));
            let user_service = user::service::user_service::MockUserService::default();
            let service = UserSearchFilterServiceImpl::new(&repository, &user_service);
            let actual = service
                .delete_user_search_filter(
                    &super::system_ctx(),
                    &UserId::new(),
                    &UserSearchFilterId::new(),
                )
                .await;

            assert!(actual.is_err());
            match actual.unwrap_err() {
                UserSearchFilterError::SdkGetItemError(_) => {}
                _ => panic!("expected SearchFilterError::SdkGetItemError"),
            }
        }

        #[tokio::test]
        #[rstest::rstest]
        #[case::construction_failure(SdkError::construction_failure("Something went wrong"))]
        #[case::timeout(SdkError::timeout_error("Something went wrong"))]
        #[case::dispatch_failure(SdkError::dispatch_failure(ConnectorError::user("Something went wrong".into())))]
        #[case::response_error(SdkError::response_error(
            "Something went wrong",
            HttpResponse::new(500u16.try_into().unwrap(), "{}".into())
        ))]
        #[case::service_error(SdkError::service_error(
            aws_sdk_dynamodb::operation::delete_item::DeleteItemError::unhandled("Something went wrong"),
            HttpResponse::new(500u16.try_into().unwrap(), "{}".into())
        ))]
        #[trace]
        async fn should_propagate_sdk_error_for_delete(
            #[case] expected: SdkError<
                aws_sdk_dynamodb::operation::delete_item::DeleteItemError,
                aws_sdk_dynamodb::config::http::HttpResponse,
            >,
        ) {
            let mut repository = MockUserSearchFilterDynamoDbRepository::default();
            repository
                .expect_get_user_search_filter_record()
                .return_once(|_, _| Box::pin(async { Ok(Some(Faker.fake())) }));
            repository
                .expect_delete_user_search_filter_record()
                .return_once(|_, _| Box::pin(async { Err(expected) }));
            let user_service = user::service::user_service::MockUserService::default();
            let service = UserSearchFilterServiceImpl::new(&repository, &user_service);
            let actual = service
                .delete_user_search_filter(
                    &super::system_ctx(),
                    &UserId::new(),
                    &UserSearchFilterId::new(),
                )
                .await;

            assert!(actual.is_err());
            match actual.unwrap_err() {
                UserSearchFilterError::SdkDeleteItemError(_) => {}
                _ => panic!("expected SearchFilterError::SdkDeleteItemError"),
            }
        }
    }

    mod update_search_filter {
        use super::FixedEmbeddingGenerator;
        use crate::core::user_search_filter_update::UserSearchFilterUpdate;
        use crate::dynamodb::repository::MockUserSearchFilterDynamoDbRepository;
        use crate::dynamodb::user_search_filter_record::UserSearchFilterRecord;
        use crate::service::user_search_filter_service::{
            UserSearchFilterError, UserSearchFilterService, UserSearchFilterServiceImpl,
        };
        use aws_sdk_dynamodb::{
            config::http::HttpResponse,
            error::{ConnectorError, SdkError},
        };
        use common::user_id::UserId;
        use common::user_search_filter_id::UserSearchFilterId;
        use fake::{Fake, Faker};
        use user::core::tier::UserTier;
        use user::core::user::User;

        #[tokio::test]
        async fn should_update_search_filter_when_exists() {
            let mut repository = MockUserSearchFilterDynamoDbRepository::default();
            repository
                .expect_get_user_search_filter_record()
                .return_once(|_, _| Box::pin(async { Ok(Some(Faker.fake())) }));
            repository
                .expect_update_user_search_filter_record()
                .return_once(|_, _, _| Box::pin(async { Ok(Some(Faker.fake())) }));
            let mut user_service = user::service::user_service::MockUserService::default();
            user_service
                .expect_find_user()
                .return_once(|_| Box::pin(async { Ok(fake::Fake::fake(&fake::Faker)) }));
            let service = UserSearchFilterServiceImpl::new(&repository, &user_service);
            let actual = service
                .update_user_search_filter(
                    &super::system_ctx(),
                    &UserId::new(),
                    &UserSearchFilterId::new(),
                    Faker.fake(),
                )
                .await;
            assert!(actual.is_ok());
        }

        #[tokio::test]
        async fn should_update_embedding_when_search_text_changes() {
            let user_id = UserId::new();
            let search_filter_id = UserSearchFilterId::new();
            let mut existing = Faker.fake::<UserSearchFilterRecord>();
            existing.user_id = user_id;
            existing.user_search_filter_id = search_filter_id;
            existing.product_query = vec!["antique lamp".try_into().unwrap()];
            existing.enhanced_search_description = None;
            existing.embedding = Some(vec![0.9]);

            let mut updated = existing.clone();
            updated.product_query = vec!["antique chandelier".try_into().unwrap()];
            updated.embedding = Some(vec![1.0 / 768_f32.sqrt(); 768]);

            let mut repository = MockUserSearchFilterDynamoDbRepository::default();
            repository
                .expect_get_user_search_filter_record()
                .return_once(move |_, _| Box::pin(async move { Ok(Some(existing)) }));
            repository
                .expect_update_user_search_filter_record()
                .return_once(move |_, _, record_update| {
                    assert_eq!(
                        record_update.embedding,
                        Some(vec![1.0 / 768_f32.sqrt(); 768])
                    );
                    Box::pin(async move { Ok(Some(updated)) })
                });

            let mut user_service = user::service::user_service::MockUserService::default();
            user_service.expect_find_user().return_once(|_| {
                Box::pin(async {
                    let mut user: User = fake::Fake::fake(&fake::Faker);
                    user.tier = UserTier::Ultimate;
                    Ok(user)
                })
            });
            let embedding_service = FixedEmbeddingGenerator(vec![1.0; 768]);

            let service = UserSearchFilterServiceImpl::with_embedding_service(
                &repository,
                &user_service,
                &embedding_service,
            );
            let update = UserSearchFilterUpdate {
                product_query: Some(vec!["antique chandelier".try_into().unwrap()]),
                ..Default::default()
            };
            let actual = service
                .update_user_search_filter(
                    &super::system_ctx(),
                    &user_id,
                    &search_filter_id,
                    update,
                )
                .await
                .unwrap();

            assert_eq!(actual.embedding, Some(vec![1.0 / 768_f32.sqrt(); 768]));
        }

        #[tokio::test]
        async fn should_return_search_filter_not_found_error_when_not_exists() {
            let mut repository = MockUserSearchFilterDynamoDbRepository::default();
            repository
                .expect_get_user_search_filter_record()
                .return_once(|_, _| Box::pin(async { Ok(None) }));
            let mut user_service = user::service::user_service::MockUserService::default();
            user_service
                .expect_find_user()
                .return_once(|_| Box::pin(async { Ok(fake::Fake::fake(&fake::Faker)) }));
            let service = UserSearchFilterServiceImpl::new(&repository, &user_service);

            let user_id = UserId::new();
            let user_search_filter_id = UserSearchFilterId::new();
            let actual = service
                .update_user_search_filter(
                    &super::system_ctx(),
                    &user_id,
                    &user_search_filter_id,
                    Faker.fake(),
                )
                .await;

            assert!(actual.is_err());
            match actual.unwrap_err() {
                UserSearchFilterError::UserSearchFilterNotFound(
                    err_user_id,
                    err_user_search_filter_id,
                ) => {
                    assert_eq!(err_user_id, user_id);
                    assert_eq!(err_user_search_filter_id, user_search_filter_id);
                }
                _ => panic!("expected SearchFilterError::SearchFilterNotFound"),
            }
        }

        #[tokio::test]
        #[rstest::rstest]
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
        #[trace]
        async fn should_propagate_sdk_error_for_find(
            #[case] expected: SdkError<
                aws_sdk_dynamodb::operation::get_item::GetItemError,
                aws_sdk_dynamodb::config::http::HttpResponse,
            >,
        ) {
            let mut repository = MockUserSearchFilterDynamoDbRepository::default();
            repository
                .expect_get_user_search_filter_record()
                .return_once(|_, _| Box::pin(async { Err(expected) }));
            let mut user_service = user::service::user_service::MockUserService::default();
            user_service
                .expect_find_user()
                .return_once(|_| Box::pin(async { Ok(fake::Fake::fake(&fake::Faker)) }));
            let service = UserSearchFilterServiceImpl::new(&repository, &user_service);
            let actual = service
                .update_user_search_filter(
                    &super::system_ctx(),
                    &UserId::new(),
                    &UserSearchFilterId::new(),
                    Faker.fake(),
                )
                .await;

            assert!(actual.is_err());
            match actual.unwrap_err() {
                UserSearchFilterError::SdkGetItemError(_) => {}
                _ => panic!("expected SearchFilterError::SdkGetItemError"),
            }
        }

        #[tokio::test]
        #[rstest::rstest]
        #[case::construction_failure(SdkError::construction_failure("Something went wrong"))]
        #[case::timeout(SdkError::timeout_error("Something went wrong"))]
        #[case::dispatch_failure(SdkError::dispatch_failure(ConnectorError::user("Something went wrong".into())))]
        #[case::response_error(SdkError::response_error(
            "Something went wrong",
            HttpResponse::new(500u16.try_into().unwrap(), "{}".into())
        ))]
        #[case::service_error(SdkError::service_error(
            aws_sdk_dynamodb::operation::update_item::UpdateItemError::unhandled("Something went wrong"),
            HttpResponse::new(500u16.try_into().unwrap(), "{}".into())
        ))]
        #[trace]
        async fn should_propagate_sdk_error_for_update(
            #[case] expected: SdkError<
                aws_sdk_dynamodb::operation::update_item::UpdateItemError,
                aws_sdk_dynamodb::config::http::HttpResponse,
            >,
        ) {
            let mut repository = MockUserSearchFilterDynamoDbRepository::default();
            repository
                .expect_get_user_search_filter_record()
                .return_once(|_, _| Box::pin(async { Ok(Some(Faker.fake())) }));
            repository
                .expect_update_user_search_filter_record()
                .return_once(|_, _, _| Box::pin(async { Err(expected) }));
            let mut user_service = user::service::user_service::MockUserService::default();
            user_service
                .expect_find_user()
                .return_once(|_| Box::pin(async { Ok(fake::Fake::fake(&fake::Faker)) }));
            let service = UserSearchFilterServiceImpl::new(&repository, &user_service);
            let actual = service
                .update_user_search_filter(
                    &super::system_ctx(),
                    &UserId::new(),
                    &UserSearchFilterId::new(),
                    Faker.fake(),
                )
                .await;

            assert!(actual.is_err());
            match actual.unwrap_err() {
                UserSearchFilterError::SdkUpdateItemError(_) => {}
                _ => panic!("expected SearchFilterError::SdkUpdateItemError"),
            }
        }

        #[tokio::test]
        async fn should_err_search_filter_feature_forbidden_when_free_tier_updates_with_forbidden_features()
         {
            use crate::core::quota::SearchFilterQuota;
            use crate::core::user_search_filter_update::UserSearchFilterUpdate;
            use user::core::user::User;

            let mut repository = MockUserSearchFilterDynamoDbRepository::default();
            repository
                .expect_get_user_search_filter_record()
                .return_once(|_, _| Box::pin(async { Ok(Some(Faker.fake())) }));
            let mut user_service = user::service::user_service::MockUserService::default();
            user_service.expect_find_user().return_once(|_| {
                Box::pin(async {
                    let mut user: User = fake::Fake::fake(&fake::Faker);
                    user.tier = user::core::tier::UserTier::Free;
                    Ok(user)
                })
            });
            let service = UserSearchFilterServiceImpl::new(&repository, &user_service);

            // Generate updates until we find one with forbidden features for Free tier
            let update: UserSearchFilterUpdate = (0..100)
                .find_map(|_| {
                    let u: UserSearchFilterUpdate = Faker.fake();
                    if user::core::tier::UserTier::Free
                        .check_search_filter_update_features(&u)
                        .is_err()
                    {
                        Some(u)
                    } else {
                        None
                    }
                })
                .expect("Should generate an update with forbidden features");

            let actual = service
                .update_user_search_filter(
                    &super::system_ctx(),
                    &UserId::new(),
                    &UserSearchFilterId::new(),
                    update,
                )
                .await
                .unwrap_err();

            match actual {
                UserSearchFilterError::SearchFilterFeatureForbidden(_) => {}
                err => panic!(
                    "Expected 'UserSearchFilterError::SearchFilterFeatureForbidden' but got '{err}'"
                ),
            }
        }

        #[tokio::test]
        async fn should_allow_ultimate_tier_to_update_with_all_features() {
            use user::core::user::User;

            let mut repository = MockUserSearchFilterDynamoDbRepository::default();
            repository
                .expect_get_user_search_filter_record()
                .return_once(|_, _| Box::pin(async { Ok(Some(Faker.fake())) }));
            repository
                .expect_update_user_search_filter_record()
                .return_once(|_, _, _| Box::pin(async { Ok(Some(Faker.fake())) }));
            let mut user_service = user::service::user_service::MockUserService::default();
            user_service.expect_find_user().return_once(|_| {
                Box::pin(async {
                    let mut user: User = fake::Fake::fake(&fake::Faker);
                    user.tier = user::core::tier::UserTier::Ultimate;
                    Ok(user)
                })
            });
            let service = UserSearchFilterServiceImpl::new(&repository, &user_service);

            let actual = service
                .update_user_search_filter(
                    &super::system_ctx(),
                    &UserId::new(),
                    &UserSearchFilterId::new(),
                    Faker.fake(),
                )
                .await;

            assert!(actual.is_ok());
        }
    }

    mod match_user_search_filters {
        use crate::dynamodb::repository::MockUserSearchFilterDynamoDbRepository;
        use crate::opensearch::repository::MockUserSearchFilterOpenSearchRepository;
        use crate::opensearch::user_search_filter_document::UserSearchFilterDocument;
        use crate::service::user_search_filter_service::{
            UserSearchFilterError, UserSearchFilterService, UserSearchFilterServiceImpl,
        };
        use fake::{Fake, Faker};
        use product::opensearch::product_document::ProductDocument;

        #[tokio::test]
        async fn should_return_matching_filters_when_product_matches_multiple_filters() {
            let mut opensearch_repo = MockUserSearchFilterOpenSearchRepository::default();
            let expected_count = 3;
            opensearch_repo.expect_percolate().return_once(move |_| {
                Box::pin(async move { Ok(fake::vec![UserSearchFilterDocument; expected_count]) })
            });

            let dynamodb_repo =
                crate::dynamodb::repository::MockUserSearchFilterDynamoDbRepository::default();
            let user_service = user::service::user_service::MockUserService::default();
            let service = UserSearchFilterServiceImpl::with_opensearch(
                &dynamodb_repo,
                &user_service,
                &opensearch_repo,
            );
            let product_document: ProductDocument = Faker.fake();

            let actual = service.match_user_search_filters(&product_document).await;

            assert!(actual.is_ok());
            let matched_filters = actual.unwrap();
            assert_eq!(matched_filters.len(), expected_count);
        }

        #[tokio::test]
        async fn should_return_single_matching_filter_when_product_matches_one_filter() {
            let mut opensearch_repo = MockUserSearchFilterOpenSearchRepository::default();
            let expected_document: UserSearchFilterDocument = Faker.fake();
            let expected_summary_id = expected_document.user_search_filter_id;

            opensearch_repo
                .expect_percolate()
                .return_once(|_| Box::pin(async move { Ok(vec![expected_document]) }));

            let dynamodb_repo =
                crate::dynamodb::repository::MockUserSearchFilterDynamoDbRepository::default();
            let user_service = user::service::user_service::MockUserService::default();
            let service = UserSearchFilterServiceImpl::with_opensearch(
                &dynamodb_repo,
                &user_service,
                &opensearch_repo,
            );
            let product_document: ProductDocument = Faker.fake();

            let actual = service.match_user_search_filters(&product_document).await;

            assert!(actual.is_ok());
            let matched_filters = actual.unwrap();
            assert_eq!(matched_filters.len(), 1);
            assert_eq!(
                matched_filters[0].user_search_filter_id,
                expected_summary_id
            );
        }

        #[tokio::test]
        async fn should_return_empty_list_when_product_matches_no_filters() {
            let mut opensearch_repo = MockUserSearchFilterOpenSearchRepository::default();
            opensearch_repo
                .expect_percolate()
                .return_once(|_| Box::pin(async { Ok(Vec::new()) }));

            let dynamodb_repo =
                crate::dynamodb::repository::MockUserSearchFilterDynamoDbRepository::default();
            let user_service = user::service::user_service::MockUserService::default();
            let service = UserSearchFilterServiceImpl::with_opensearch(
                &dynamodb_repo,
                &user_service,
                &opensearch_repo,
            );
            let product_document: ProductDocument = Faker.fake();

            let actual = service.match_user_search_filters(&product_document).await;

            assert!(actual.is_ok());
            let matched_filters = actual.unwrap();
            assert!(matched_filters.is_empty());
        }

        #[tokio::test]
        async fn should_return_error_when_opensearch_repository_not_configured() {
            let dynamodb_repo =
                crate::dynamodb::repository::MockUserSearchFilterDynamoDbRepository::default();
            let user_service = user::service::user_service::MockUserService::default();
            let service = UserSearchFilterServiceImpl::new(&dynamodb_repo, &user_service);
            let product_document: ProductDocument = Faker.fake();

            let actual = service.match_user_search_filters(&product_document).await;

            assert!(actual.is_err());
            match actual.unwrap_err() {
                UserSearchFilterError::OpenSearchError(_) => {}
                other => panic!(
                    "expected UserSearchFilterError::OpenSearchError, but got: {:?}",
                    other
                ),
            }
        }

        #[tokio::test]
        async fn should_propagate_opensearch_error_when_percolate_fails() {
            let mut opensearch_repo = MockUserSearchFilterOpenSearchRepository::default();
            use serde::ser::Error as _;
            let expected_error =
                opensearch::Error::from(serde_json::Error::custom("Percolate query failed"));
            opensearch_repo
                .expect_percolate()
                .return_once(|_| Box::pin(async { Err(expected_error) }));

            let dynamodb_repo =
                crate::dynamodb::repository::MockUserSearchFilterDynamoDbRepository::default();
            let user_service = user::service::user_service::MockUserService::default();
            let service = UserSearchFilterServiceImpl::with_opensearch(
                &dynamodb_repo,
                &user_service,
                &opensearch_repo,
            );
            let product_document: ProductDocument = Faker.fake();

            let actual = service.match_user_search_filters(&product_document).await;

            assert!(actual.is_err());
            match actual.unwrap_err() {
                UserSearchFilterError::OpenSearchError(_) => {}
                other => panic!(
                    "expected UserSearchFilterError::OpenSearchError, but got: {:?}",
                    other
                ),
            }
        }

        #[rstest::rstest]
        #[case::empty(0)]
        #[case::single(1)]
        #[case::multiple(10)]
        #[case::large_batch(100)]
        #[tokio::test]
        #[trace]
        async fn should_convert_opensearch_documents_to_summaries(#[case] count: usize) {
            let mut opensearch_repo = MockUserSearchFilterOpenSearchRepository::default();
            let documents: Vec<UserSearchFilterDocument> =
                fake::vec![UserSearchFilterDocument; count];
            let expected_ids: Vec<_> = documents.iter().map(|d| d.user_search_filter_id).collect();

            opensearch_repo
                .expect_percolate()
                .return_once(move |_| Box::pin(async move { Ok(documents) }));

            let dynamodb_repo =
                crate::dynamodb::repository::MockUserSearchFilterDynamoDbRepository::default();
            let user_service = user::service::user_service::MockUserService::default();
            let service = UserSearchFilterServiceImpl::with_opensearch(
                &dynamodb_repo,
                &user_service,
                &opensearch_repo,
            );
            let product_document: ProductDocument = Faker.fake();

            let actual = service.match_user_search_filters(&product_document).await;

            assert!(actual.is_ok());
            let matched_filters = actual.unwrap();
            assert_eq!(matched_filters.len(), count);
            for (i, filter) in matched_filters.iter().enumerate() {
                assert_eq!(filter.user_search_filter_id, expected_ids[i]);
            }
        }

        #[tokio::test]
        async fn should_preserve_user_id_when_converting_matching_documents() {
            let mut opensearch_repo = MockUserSearchFilterOpenSearchRepository::default();
            let expected_document: UserSearchFilterDocument = Faker.fake();
            let expected_user_id = expected_document.user_id;

            opensearch_repo
                .expect_percolate()
                .return_once(|_| Box::pin(async move { Ok(vec![expected_document]) }));

            let dynamodb_repo =
                crate::dynamodb::repository::MockUserSearchFilterDynamoDbRepository::default();
            let user_service = user::service::user_service::MockUserService::default();
            let service = UserSearchFilterServiceImpl::with_opensearch(
                &dynamodb_repo,
                &user_service,
                &opensearch_repo,
            );
            let product_document: ProductDocument = Faker.fake();

            let actual = service.match_user_search_filters(&product_document).await;

            assert!(actual.is_ok());
            let matched_filters = actual.unwrap();
            assert_eq!(matched_filters[0].user_id, expected_user_id);
        }

        #[tokio::test]
        async fn should_preserve_filter_name_when_converting_matching_documents() {
            let mut opensearch_repo = MockUserSearchFilterOpenSearchRepository::default();
            let expected_document: UserSearchFilterDocument = Faker.fake();
            let expected_name = expected_document.name.clone();

            opensearch_repo
                .expect_percolate()
                .return_once(|_| Box::pin(async move { Ok(vec![expected_document]) }));

            let dynamodb_repo =
                crate::dynamodb::repository::MockUserSearchFilterDynamoDbRepository::default();
            let user_service = user::service::user_service::MockUserService::default();
            let service = UserSearchFilterServiceImpl::with_opensearch(
                &dynamodb_repo,
                &user_service,
                &opensearch_repo,
            );
            let product_document: ProductDocument = Faker.fake();

            let actual = service.match_user_search_filters(&product_document).await;

            assert!(actual.is_ok());
            let matched_filters = actual.unwrap();
            assert_eq!(matched_filters[0].name, expected_name);
        }

        #[tokio::test]
        async fn should_preserve_timestamps_when_converting_matching_documents() {
            let mut opensearch_repo = MockUserSearchFilterOpenSearchRepository::default();
            let expected_document: UserSearchFilterDocument = Faker.fake();
            let expected_created = expected_document.created;
            let expected_updated = expected_document.updated;

            opensearch_repo
                .expect_percolate()
                .return_once(|_| Box::pin(async move { Ok(vec![expected_document]) }));

            let dynamodb_repo =
                crate::dynamodb::repository::MockUserSearchFilterDynamoDbRepository::default();
            let user_service = user::service::user_service::MockUserService::default();
            let service = UserSearchFilterServiceImpl::with_opensearch(
                &dynamodb_repo,
                &user_service,
                &opensearch_repo,
            );
            let product_document: ProductDocument = Faker.fake();

            let actual = service.match_user_search_filters(&product_document).await;

            assert!(actual.is_ok());
            let matched_filters = actual.unwrap();
            assert_eq!(matched_filters[0].created, expected_created);
            assert_eq!(matched_filters[0].updated, expected_updated);
        }

        #[tokio::test]
        async fn should_accept_product_document_reference_without_consuming_it() {
            let mut opensearch_repo = MockUserSearchFilterOpenSearchRepository::default();
            let matched_document: UserSearchFilterDocument = Faker.fake();
            let document_clone = matched_document.clone();

            opensearch_repo
                .expect_percolate()
                .return_once(|_| Box::pin(async move { Ok(vec![matched_document]) }));

            let dynamodb_repo =
                crate::dynamodb::repository::MockUserSearchFilterDynamoDbRepository::default();
            let user_service = user::service::user_service::MockUserService::default();
            let service = UserSearchFilterServiceImpl::with_opensearch(
                &dynamodb_repo,
                &user_service,
                &opensearch_repo,
            );
            let product_document: ProductDocument = Faker.fake();

            // Call the method with product_document reference
            let result = service.match_user_search_filters(&product_document).await;

            // Verify the result is OK and product_document reference wasn't consumed
            assert!(result.is_ok());
            let matched_filters = result.unwrap();
            assert_eq!(matched_filters.len(), 1);
            assert_eq!(
                matched_filters[0].user_search_filter_id,
                document_clone.user_search_filter_id
            );
        }

        #[tokio::test]
        async fn should_handle_large_batch_of_matching_filters() {
            let mut opensearch_repo = MockUserSearchFilterOpenSearchRepository::default();
            let documents: Vec<UserSearchFilterDocument> =
                fake::vec![UserSearchFilterDocument; 500];
            let expected_count = documents.len();

            opensearch_repo
                .expect_percolate()
                .return_once(move |_| Box::pin(async move { Ok(documents) }));

            let dynamodb_repo = MockUserSearchFilterDynamoDbRepository::default();
            let user_service = user::service::user_service::MockUserService::default();
            let service = UserSearchFilterServiceImpl::with_opensearch(
                &dynamodb_repo,
                &user_service,
                &opensearch_repo,
            );
            let product_document: ProductDocument = Faker.fake();

            let actual = service.match_user_search_filters(&product_document).await;

            assert!(actual.is_ok());
            let matched_filters = actual.unwrap();
            assert_eq!(matched_filters.len(), expected_count);
        }

        #[tokio::test]
        async fn should_maintain_filter_order_from_opensearch_results() {
            let mut opensearch_repo = MockUserSearchFilterOpenSearchRepository::default();
            let documents: Vec<UserSearchFilterDocument> = fake::vec![UserSearchFilterDocument; 5];
            let expected_ids: Vec<_> = documents.iter().map(|d| d.user_search_filter_id).collect();

            opensearch_repo
                .expect_percolate()
                .return_once(move |_| Box::pin(async move { Ok(documents) }));

            let dynamodb_repo = MockUserSearchFilterDynamoDbRepository::default();
            let user_service = user::service::user_service::MockUserService::default();
            let service = UserSearchFilterServiceImpl::with_opensearch(
                &dynamodb_repo,
                &user_service,
                &opensearch_repo,
            );
            let product_document: ProductDocument = Faker.fake();

            let actual = service.match_user_search_filters(&product_document).await;

            assert!(actual.is_ok());
            let matched_filters = actual.unwrap();
            for (i, summary) in matched_filters.iter().enumerate() {
                assert_eq!(summary.user_search_filter_id, expected_ids[i]);
            }
        }

        #[tokio::test]
        async fn should_correctly_handle_single_filter_with_all_fields() {
            let mut opensearch_repo = MockUserSearchFilterOpenSearchRepository::default();
            let expected_document: UserSearchFilterDocument = Faker.fake();
            let expected_id = expected_document.user_search_filter_id;
            let expected_user_id = expected_document.user_id;
            let expected_name = expected_document.name.clone();
            let expected_created = expected_document.created;
            let expected_updated = expected_document.updated;

            opensearch_repo
                .expect_percolate()
                .return_once(|_| Box::pin(async move { Ok(vec![expected_document]) }));

            let dynamodb_repo = MockUserSearchFilterDynamoDbRepository::default();
            let user_service = user::service::user_service::MockUserService::default();
            let service = UserSearchFilterServiceImpl::with_opensearch(
                &dynamodb_repo,
                &user_service,
                &opensearch_repo,
            );
            let product_document: ProductDocument = Faker.fake();

            let actual = service.match_user_search_filters(&product_document).await;

            assert!(actual.is_ok());
            let matched_filters = actual.unwrap();
            assert_eq!(matched_filters.len(), 1);

            let summary = &matched_filters[0];
            assert_eq!(summary.user_search_filter_id, expected_id);
            assert_eq!(summary.user_id, expected_user_id);
            assert_eq!(summary.name, expected_name);
            assert_eq!(summary.created, expected_created);
            assert_eq!(summary.updated, expected_updated);
        }

        #[tokio::test]
        async fn should_return_summaries_with_all_required_fields() {
            let mut opensearch_repo = MockUserSearchFilterOpenSearchRepository::default();
            let documents = vec![Faker.fake::<UserSearchFilterDocument>()];

            opensearch_repo
                .expect_percolate()
                .return_once(move |_| Box::pin(async move { Ok(documents) }));

            let dynamodb_repo = MockUserSearchFilterDynamoDbRepository::default();
            let user_service = user::service::user_service::MockUserService::default();
            let service = UserSearchFilterServiceImpl::with_opensearch(
                &dynamodb_repo,
                &user_service,
                &opensearch_repo,
            );
            let product_document: ProductDocument = Faker.fake();

            let actual = service.match_user_search_filters(&product_document).await;

            assert!(actual.is_ok());
            let matched_filters = actual.unwrap();
            assert_eq!(matched_filters.len(), 1);

            let summary = &matched_filters[0];
            assert!(!summary.user_search_filter_id.to_string().is_empty());
            assert!(!summary.user_id.to_string().is_empty());
            assert!(!summary.name.to_string().is_empty());
        }

        #[tokio::test]
        async fn should_not_modify_product_document_when_matching() {
            let mut opensearch_repo = MockUserSearchFilterOpenSearchRepository::default();
            let product_document_copy = Faker.fake::<ProductDocument>();

            opensearch_repo
                .expect_percolate()
                .return_once(|_| Box::pin(async { Ok(Vec::new()) }));

            let dynamodb_repo = MockUserSearchFilterDynamoDbRepository::default();
            let user_service = user::service::user_service::MockUserService::default();
            let service = UserSearchFilterServiceImpl::with_opensearch(
                &dynamodb_repo,
                &user_service,
                &opensearch_repo,
            );
            let product_document = product_document_copy.clone();

            let _ = service.match_user_search_filters(&product_document).await;

            assert_eq!(
                product_document.product_id,
                product_document_copy.product_id
            );
            assert_eq!(product_document.shop_id, product_document_copy.shop_id);
        }

        #[tokio::test]
        async fn should_correctly_convert_all_fields_in_result() {
            let mut opensearch_repo = MockUserSearchFilterOpenSearchRepository::default();
            let mut documents = vec![];
            for _ in 0..3 {
                documents.push(Faker.fake::<UserSearchFilterDocument>());
            }

            let original_documents = documents.clone();
            opensearch_repo
                .expect_percolate()
                .return_once(move |_| Box::pin(async move { Ok(documents) }));

            let dynamodb_repo = MockUserSearchFilterDynamoDbRepository::default();
            let user_service = user::service::user_service::MockUserService::default();
            let service = UserSearchFilterServiceImpl::with_opensearch(
                &dynamodb_repo,
                &user_service,
                &opensearch_repo,
            );
            let product_document: ProductDocument = Faker.fake();

            let actual = service.match_user_search_filters(&product_document).await;

            assert!(actual.is_ok());
            let matched_filters = actual.unwrap();

            for (i, summary) in matched_filters.iter().enumerate() {
                assert_eq!(
                    summary.user_search_filter_id,
                    original_documents[i].user_search_filter_id
                );
                assert_eq!(summary.user_id, original_documents[i].user_id);
                assert_eq!(summary.name, original_documents[i].name);
                assert_eq!(summary.created, original_documents[i].created);
                assert_eq!(summary.updated, original_documents[i].updated);
            }
        }

        #[tokio::test]
        async fn should_return_result_type_on_success() {
            let mut opensearch_repo = MockUserSearchFilterOpenSearchRepository::default();
            opensearch_repo
                .expect_percolate()
                .return_once(|_| Box::pin(async { Ok(Vec::new()) }));

            let dynamodb_repo = MockUserSearchFilterDynamoDbRepository::default();
            let user_service = user::service::user_service::MockUserService::default();
            let service = UserSearchFilterServiceImpl::with_opensearch(
                &dynamodb_repo,
                &user_service,
                &opensearch_repo,
            );
            let product_document: ProductDocument = Faker.fake();

            let actual = service.match_user_search_filters(&product_document).await;

            assert!(actual.is_ok());
            assert!(actual.unwrap().is_empty());
        }

        #[tokio::test]
        async fn should_return_error_type_on_missing_repository() {
            let dynamodb_repo = MockUserSearchFilterDynamoDbRepository::default();
            let user_service = user::service::user_service::MockUserService::default();
            let service = UserSearchFilterServiceImpl::new(&dynamodb_repo, &user_service);
            let product_document: ProductDocument = Faker.fake();

            let result = service.match_user_search_filters(&product_document).await;

            assert!(result.is_err());
            assert!(matches!(
                result.unwrap_err(),
                UserSearchFilterError::OpenSearchError(_)
            ));
        }

        #[tokio::test]
        async fn should_handle_multiple_documents_with_different_user_ids() {
            let mut opensearch_repo = MockUserSearchFilterOpenSearchRepository::default();
            let documents: Vec<UserSearchFilterDocument> = fake::vec![UserSearchFilterDocument; 4];
            let user_ids: Vec<_> = documents.iter().map(|d| d.user_id).collect();

            opensearch_repo
                .expect_percolate()
                .return_once(move |_| Box::pin(async move { Ok(documents) }));

            let dynamodb_repo = MockUserSearchFilterDynamoDbRepository::default();
            let user_service = user::service::user_service::MockUserService::default();
            let service = UserSearchFilterServiceImpl::with_opensearch(
                &dynamodb_repo,
                &user_service,
                &opensearch_repo,
            );
            let product_document: ProductDocument = Faker.fake();

            let actual = service.match_user_search_filters(&product_document).await;

            assert!(actual.is_ok());
            let matched_filters = actual.unwrap();
            for (i, summary) in matched_filters.iter().enumerate() {
                assert_eq!(summary.user_id, user_ids[i]);
            }
        }

        #[tokio::test]
        async fn should_properly_map_opensearch_document_to_summary_type() {
            let mut opensearch_repo = MockUserSearchFilterOpenSearchRepository::default();
            let doc: UserSearchFilterDocument = Faker.fake();
            let doc_id = doc.user_search_filter_id;
            let doc_user_id = doc.user_id;
            let doc_name = doc.name.clone();

            opensearch_repo
                .expect_percolate()
                .return_once(|_| Box::pin(async move { Ok(vec![doc]) }));

            let dynamodb_repo = MockUserSearchFilterDynamoDbRepository::default();
            let user_service = user::service::user_service::MockUserService::default();
            let service = UserSearchFilterServiceImpl::with_opensearch(
                &dynamodb_repo,
                &user_service,
                &opensearch_repo,
            );
            let product_document: ProductDocument = Faker.fake();

            let actual = service.match_user_search_filters(&product_document).await;

            assert!(actual.is_ok());
            let summaries = actual.unwrap();
            assert_eq!(summaries.len(), 1);

            // Verify that the type conversion happened correctly
            let summary = &summaries[0];
            assert_eq!(summary.user_search_filter_id, doc_id);
            assert_eq!(summary.user_id, doc_user_id);
            assert_eq!(summary.name, doc_name);
        }

        #[tokio::test]
        async fn should_handle_percolate_returning_exact_count() {
            let mut opensearch_repo = MockUserSearchFilterOpenSearchRepository::default();
            let count = 7;
            let documents: Vec<UserSearchFilterDocument> =
                fake::vec![UserSearchFilterDocument; count];

            opensearch_repo
                .expect_percolate()
                .return_once(move |_| Box::pin(async move { Ok(documents) }));

            let dynamodb_repo = MockUserSearchFilterDynamoDbRepository::default();
            let user_service = user::service::user_service::MockUserService::default();
            let service = UserSearchFilterServiceImpl::with_opensearch(
                &dynamodb_repo,
                &user_service,
                &opensearch_repo,
            );
            let product_document: ProductDocument = Faker.fake();

            let actual = service.match_user_search_filters(&product_document).await;

            assert!(actual.is_ok());
            let matched_filters = actual.unwrap();
            assert_eq!(matched_filters.len(), count);
        }
    }

    mod find_search_filter_product_match {
        use crate::dynamodb::repository::MockUserSearchFilterDynamoDbRepository;
        use crate::dynamodb::user_search_filter_match_record::UserSearchFilterMatchRecord;
        use crate::service::user_search_filter_service::{
            UserSearchFilterService, UserSearchFilterServiceImpl,
        };
        use common::shop_id::ShopId;
        use common::shops_product_id::ShopsProductId;
        use common::user_id::UserId;
        use common::user_search_filter_id::UserSearchFilterId;
        use fake::{Fake, Faker};

        #[tokio::test]
        async fn should_return_none_when_no_match_exists() {
            let mut repository = MockUserSearchFilterDynamoDbRepository::default();
            repository
                .expect_get_user_search_filter_match_record()
                .return_once(|_, _, _, _| Box::pin(async { Ok(None) }));
            let user_service = user::service::user_service::MockUserService::default();
            let service = UserSearchFilterServiceImpl::new(&repository, &user_service);

            let actual = service
                .find_search_filter_product_match(
                    &UserId::new(),
                    &UserSearchFilterId::new(),
                    &ShopId::new(),
                    &ShopsProductId::new(),
                )
                .await;
            assert!(actual.is_ok());
            assert!(actual.unwrap().is_none());
        }

        #[tokio::test]
        async fn should_return_some_when_match_exists() {
            let mut repository = MockUserSearchFilterDynamoDbRepository::default();
            repository
                .expect_get_user_search_filter_match_record()
                .return_once(|_, _, _, _| {
                    Box::pin(async { Ok(Some(Faker.fake::<UserSearchFilterMatchRecord>())) })
                });
            let user_service = user::service::user_service::MockUserService::default();
            let service = UserSearchFilterServiceImpl::new(&repository, &user_service);

            let actual = service
                .find_search_filter_product_match(
                    &UserId::new(),
                    &UserSearchFilterId::new(),
                    &ShopId::new(),
                    &ShopsProductId::new(),
                )
                .await;
            assert!(actual.is_ok());
            assert!(actual.unwrap().is_some());
        }

        #[tokio::test]
        async fn should_propagate_sdk_error() {
            use aws_sdk_dynamodb::error::SdkError;
            let mut repository = MockUserSearchFilterDynamoDbRepository::default();
            repository
                .expect_get_user_search_filter_match_record()
                .return_once(|_, _, _, _| {
                    Box::pin(async { Err(SdkError::construction_failure("test error")) })
                });
            let user_service = user::service::user_service::MockUserService::default();
            let service = UserSearchFilterServiceImpl::new(&repository, &user_service);

            let actual = service
                .find_search_filter_product_match(
                    &UserId::new(),
                    &UserSearchFilterId::new(),
                    &ShopId::new(),
                    &ShopsProductId::new(),
                )
                .await;
            assert!(actual.is_err());
        }
    }

    mod find_search_filter_product_matches {
        use crate::dynamodb::repository::MockUserSearchFilterDynamoDbRepository;
        use crate::dynamodb::user_search_filter_match_record::UserSearchFilterMatchRecord;
        use crate::service::user_search_filter_service::{
            UserSearchFilterService, UserSearchFilterServiceImpl,
        };
        use common::user_id::UserId;

        #[rstest::rstest]
        #[case::empty(0)]
        #[case::non_empty(5)]
        #[tokio::test]
        #[trace]
        async fn should_return_matches(#[case] count: usize) {
            let mut repository = MockUserSearchFilterDynamoDbRepository::default();
            repository
                .expect_query_user_search_filter_match_records_all()
                .return_once(move |_| {
                    Box::pin(async move { Ok(fake::vec![UserSearchFilterMatchRecord; count]) })
                });
            let user_service = user::service::user_service::MockUserService::default();
            let service = UserSearchFilterServiceImpl::new(&repository, &user_service);

            let actual = service
                .find_search_filter_product_matches(&UserId::new())
                .await;
            assert!(actual.is_ok());
            assert_eq!(actual.unwrap().len(), count);
        }

        #[tokio::test]
        async fn should_propagate_sdk_error() {
            use aws_sdk_dynamodb::error::SdkError;
            let mut repository = MockUserSearchFilterDynamoDbRepository::default();
            repository
                .expect_query_user_search_filter_match_records_all()
                .return_once(|_| {
                    Box::pin(async { Err(SdkError::construction_failure("test error")) })
                });
            let user_service = user::service::user_service::MockUserService::default();
            let service = UserSearchFilterServiceImpl::new(&repository, &user_service);

            let actual = service
                .find_search_filter_product_matches(&UserId::new())
                .await;
            assert!(actual.is_err());
        }
    }

    mod find_search_filter_product_matches_for_filter {
        use crate::dynamodb::repository::MockUserSearchFilterDynamoDbRepository;
        use crate::dynamodb::user_search_filter_match_record::UserSearchFilterMatchRecord;
        use crate::service::user_search_filter_service::{
            UserSearchFilterService, UserSearchFilterServiceImpl,
        };
        use common::user_id::UserId;
        use common::user_search_filter_id::UserSearchFilterId;

        #[rstest::rstest]
        #[case::empty(0)]
        #[case::non_empty(3)]
        #[tokio::test]
        #[trace]
        async fn should_return_matches_for_filter(#[case] count: usize) {
            let mut repository = MockUserSearchFilterDynamoDbRepository::default();
            repository
                .expect_query_user_search_filter_match_records_for_filter()
                .return_once(move |_, _, _, _| {
                    Box::pin(async move { Ok(fake::vec![UserSearchFilterMatchRecord; count]) })
                });
            let user_service = user::service::user_service::MockUserService::default();
            let service = UserSearchFilterServiceImpl::new(&repository, &user_service);

            let actual = service
                .find_search_filter_product_matches_for_filter(
                    &UserId::new(),
                    &UserSearchFilterId::new(),
                    &None,
                    None,
                )
                .await;
            assert!(actual.is_ok());
            assert_eq!(actual.unwrap().len(), count);
        }

        #[tokio::test]
        async fn should_propagate_sdk_error() {
            use aws_sdk_dynamodb::error::SdkError;
            let mut repository = MockUserSearchFilterDynamoDbRepository::default();
            repository
                .expect_query_user_search_filter_match_records_for_filter()
                .return_once(|_, _, _, _| {
                    Box::pin(async { Err(SdkError::construction_failure("test error")) })
                });
            let user_service = user::service::user_service::MockUserService::default();
            let service = UserSearchFilterServiceImpl::new(&repository, &user_service);

            let actual = service
                .find_search_filter_product_matches_for_filter(
                    &UserId::new(),
                    &UserSearchFilterId::new(),
                    &None,
                    None,
                )
                .await;
            assert!(actual.is_err());
        }
    }

    mod create_search_filter_product_match {
        use crate::core::search_filter_product_match::SearchFilterProductMatch;
        use crate::dynamodb::repository::MockUserSearchFilterDynamoDbRepository;
        use crate::service::user_search_filter_service::{
            UserSearchFilterService, UserSearchFilterServiceImpl,
        };
        use aws_sdk_dynamodb::operation::put_item::PutItemOutput;
        use fake::{Fake, Faker};

        #[tokio::test]
        async fn should_create_match_successfully() {
            let mut repository = MockUserSearchFilterDynamoDbRepository::default();
            repository
                .expect_put_user_search_filter_match_record()
                .return_once(|_| Box::pin(async { Ok(PutItemOutput::builder().build()) }));
            let user_service = user::service::user_service::MockUserService::default();
            let service = UserSearchFilterServiceImpl::new(&repository, &user_service);

            let product_match: SearchFilterProductMatch = Faker.fake();
            let actual = service
                .create_search_filter_product_match(&super::system_ctx(), product_match.clone())
                .await;
            assert!(actual.is_ok());
            assert_eq!(actual.unwrap(), product_match);
        }

        #[tokio::test]
        async fn should_propagate_sdk_error() {
            use aws_sdk_dynamodb::error::SdkError;
            let mut repository = MockUserSearchFilterDynamoDbRepository::default();
            repository
                .expect_put_user_search_filter_match_record()
                .return_once(|_| {
                    Box::pin(async { Err(SdkError::construction_failure("test error")) })
                });
            let user_service = user::service::user_service::MockUserService::default();
            let service = UserSearchFilterServiceImpl::new(&repository, &user_service);

            let product_match: SearchFilterProductMatch = Faker.fake();
            let actual = service
                .create_search_filter_product_match(&super::system_ctx(), product_match)
                .await;
            assert!(actual.is_err());
        }
    }

    mod update_search_filter_product_match {
        use crate::core::command::UpdateUserSearchFilterMatchCommand;
        use crate::dynamodb::repository::MockUserSearchFilterDynamoDbRepository;
        use crate::dynamodb::user_search_filter_match_record::UserSearchFilterMatchRecord;
        use crate::service::user_search_filter_service::{
            UserSearchFilterError, UserSearchFilterService, UserSearchFilterServiceImpl,
        };
        use common::shop_id::ShopId;
        use common::shops_product_id::ShopsProductId;
        use common::user_id::UserId;
        use common::user_search_filter_id::UserSearchFilterId;
        use fake::{Fake, Faker};

        #[tokio::test]
        async fn should_update_feedback_when_match_exists() {
            let user_id = UserId::new();
            let search_filter_id = UserSearchFilterId::new();
            let shop_id = ShopId::new();
            let shops_product_id = ShopsProductId::new();
            let mut updated_record = Faker.fake::<UserSearchFilterMatchRecord>();
            updated_record.feedback = Some(false);

            let mut repository = MockUserSearchFilterDynamoDbRepository::default();
            repository
                .expect_get_user_search_filter_match_record()
                .return_once(|_, _, _, _| {
                    Box::pin(async { Ok(Some(Faker.fake::<UserSearchFilterMatchRecord>())) })
                });
            repository
                .expect_update_user_search_filter_match_record()
                .return_once(move |_, _, _, _, update| {
                    assert_eq!(Some(false), update.feedback);
                    Box::pin(async move { Ok(Some(updated_record)) })
                });
            let user_service = user::service::user_service::MockUserService::default();
            let service = UserSearchFilterServiceImpl::new(&repository, &user_service);

            let actual = service
                .update_search_filter_product_match(
                    &super::system_ctx(),
                    user_id,
                    search_filter_id,
                    shop_id,
                    shops_product_id,
                    UpdateUserSearchFilterMatchCommand {
                        feedback: Some(false),
                    },
                )
                .await
                .unwrap();

            assert_eq!(Some(false), actual.feedback);
        }

        #[tokio::test]
        async fn should_return_existing_match_when_update_is_empty() {
            let existing_record = Faker.fake::<UserSearchFilterMatchRecord>();
            let expected: crate::core::search_filter_product_match::SearchFilterProductMatch =
                existing_record.clone().into();
            let mut repository = MockUserSearchFilterDynamoDbRepository::default();
            repository
                .expect_get_user_search_filter_match_record()
                .return_once(move |_, _, _, _| Box::pin(async move { Ok(Some(existing_record)) }));
            repository
                .expect_update_user_search_filter_match_record()
                .never();
            let user_service = user::service::user_service::MockUserService::default();
            let service = UserSearchFilterServiceImpl::new(&repository, &user_service);

            let actual = service
                .update_search_filter_product_match(
                    &super::system_ctx(),
                    UserId::new(),
                    UserSearchFilterId::new(),
                    ShopId::new(),
                    ShopsProductId::new(),
                    UpdateUserSearchFilterMatchCommand::default(),
                )
                .await
                .unwrap();

            assert_eq!(expected, actual);
        }

        #[tokio::test]
        async fn should_return_not_found_when_match_missing() {
            let mut repository = MockUserSearchFilterDynamoDbRepository::default();
            repository
                .expect_get_user_search_filter_match_record()
                .return_once(|_, _, _, _| Box::pin(async { Ok(None) }));
            repository
                .expect_update_user_search_filter_match_record()
                .never();
            let user_service = user::service::user_service::MockUserService::default();
            let service = UserSearchFilterServiceImpl::new(&repository, &user_service);

            let actual = service
                .update_search_filter_product_match(
                    &super::system_ctx(),
                    UserId::new(),
                    UserSearchFilterId::new(),
                    ShopId::new(),
                    ShopsProductId::new(),
                    UpdateUserSearchFilterMatchCommand {
                        feedback: Some(true),
                    },
                )
                .await;

            assert!(matches!(
                actual,
                Err(UserSearchFilterError::UserSearchFilterMatchNotFound(
                    _,
                    _,
                    _,
                    _
                ))
            ));
        }

        #[tokio::test]
        async fn should_propagate_sdk_error() {
            use aws_sdk_dynamodb::error::SdkError;

            let mut repository = MockUserSearchFilterDynamoDbRepository::default();
            repository
                .expect_get_user_search_filter_match_record()
                .return_once(|_, _, _, _| {
                    Box::pin(async { Err(SdkError::construction_failure("test error")) })
                });
            let user_service = user::service::user_service::MockUserService::default();
            let service = UserSearchFilterServiceImpl::new(&repository, &user_service);

            let actual = service
                .update_search_filter_product_match(
                    &super::system_ctx(),
                    UserId::new(),
                    UserSearchFilterId::new(),
                    ShopId::new(),
                    ShopsProductId::new(),
                    UpdateUserSearchFilterMatchCommand {
                        feedback: Some(true),
                    },
                )
                .await;

            assert!(actual.is_err());
        }
    }

    mod create_search_filter_product_matches {
        use crate::core::search_filter_product_match::SearchFilterProductMatch;
        use crate::dynamodb::repository::MockUserSearchFilterDynamoDbRepository;
        use crate::service::user_search_filter_service::{
            UserSearchFilterService, UserSearchFilterServiceImpl,
        };
        use aws_sdk_dynamodb::operation::batch_write_item::BatchWriteItemOutput;
        use fake::{Fake, Faker};

        #[tokio::test]
        async fn should_return_empty_when_no_matches() {
            let repository = MockUserSearchFilterDynamoDbRepository::default();
            let user_service = user::service::user_service::MockUserService::default();
            let service = UserSearchFilterServiceImpl::new(&repository, &user_service);

            let actual = service
                .create_search_filter_product_matches(&super::system_ctx(), vec![])
                .await;
            assert!(actual.is_ok());
            let result = actual.unwrap();
            assert!(result.processed.is_empty());
            assert!(result.unprocessed.is_empty());
        }

        #[tokio::test]
        async fn should_process_all_matches_when_batch_succeeds() {
            let mut repository = MockUserSearchFilterDynamoDbRepository::default();
            repository
                .expect_put_user_search_filter_match_records()
                .return_once(|_| Box::pin(async { Ok(BatchWriteItemOutput::builder().build()) }));
            let user_service = user::service::user_service::MockUserService::default();
            let service = UserSearchFilterServiceImpl::new(&repository, &user_service);

            let matches: Vec<SearchFilterProductMatch> = (0..3).map(|_| Faker.fake()).collect();
            let actual = service
                .create_search_filter_product_matches(&super::system_ctx(), matches)
                .await;
            assert!(actual.is_ok());
            let result = actual.unwrap();
            assert_eq!(result.processed.len(), 3);
            assert!(result.unprocessed.is_empty());
        }

        #[tokio::test]
        async fn should_mark_all_as_unprocessed_when_batch_fails() {
            use aws_sdk_dynamodb::error::SdkError;
            let mut repository = MockUserSearchFilterDynamoDbRepository::default();
            repository
                .expect_put_user_search_filter_match_records()
                .return_once(|_| {
                    Box::pin(async { Err(SdkError::construction_failure("test error")) })
                });
            let user_service = user::service::user_service::MockUserService::default();
            let service = UserSearchFilterServiceImpl::new(&repository, &user_service);

            let matches: Vec<SearchFilterProductMatch> = (0..3).map(|_| Faker.fake()).collect();
            let actual = service
                .create_search_filter_product_matches(&super::system_ctx(), matches)
                .await;
            assert!(actual.is_ok());
            let result = actual.unwrap();
            assert!(result.processed.is_empty());
            assert_eq!(result.unprocessed.len(), 3);
        }
    }

    mod view_search_filter_matches {
        use crate::dynamodb::repository::MockUserSearchFilterDynamoDbRepository;
        use crate::dynamodb::user_search_filter_match_record::UserSearchFilterMatchRecord;
        use crate::service::user_search_filter_service::{
            UserSearchFilterService, UserSearchFilterServiceImpl,
        };
        use common::user_id::UserId;
        use common::user_search_filter_id::UserSearchFilterId;

        #[tokio::test]
        async fn should_return_empty_cursored_result_when_no_matches() {
            let mut repository = MockUserSearchFilterDynamoDbRepository::default();
            repository
                .expect_query_user_search_filter_match_records_for_filter()
                .return_once(|_, _, _, _| Box::pin(async { Ok(vec![]) }));
            repository
                .expect_count_user_search_filter_match_records_for_filter()
                .return_once(|_, _, _, _| Box::pin(async { Ok(0) }));
            let user_service = user::service::user_service::MockUserService::default();
            let service = UserSearchFilterServiceImpl::new(&repository, &user_service);

            let actual = service
                .view_search_filter_matches(&UserId::new(), &UserSearchFilterId::new(), &None, None)
                .await;
            assert!(actual.is_ok());
            let result = actual.unwrap();
            assert!(result.items.is_empty());
            assert_eq!(result.total, Some(0));
            assert_eq!(result.cursor.size, 0);
            assert!(result.cursor.search_after.is_none());
        }

        #[tokio::test]
        async fn should_return_cursored_result_with_matches() {
            let mut repository = MockUserSearchFilterDynamoDbRepository::default();
            repository
                .expect_query_user_search_filter_match_records_for_filter()
                .return_once(|_, _, _, _| {
                    Box::pin(async { Ok(fake::vec![UserSearchFilterMatchRecord; 3]) })
                });
            repository
                .expect_count_user_search_filter_match_records_for_filter()
                .return_once(|_, _, _, _| Box::pin(async { Ok(5) }));
            let user_service = user::service::user_service::MockUserService::default();
            let service = UserSearchFilterServiceImpl::new(&repository, &user_service);

            let actual = service
                .view_search_filter_matches(&UserId::new(), &UserSearchFilterId::new(), &None, None)
                .await;
            assert!(actual.is_ok());
            let result = actual.unwrap();
            assert_eq!(result.items.len(), 3);
            assert_eq!(result.total, Some(5));
            assert_eq!(result.cursor.size, 3);
            assert!(result.cursor.search_after.is_some());
        }

        #[tokio::test]
        async fn should_propagate_sdk_error() {
            use aws_sdk_dynamodb::error::SdkError;
            let mut repository = MockUserSearchFilterDynamoDbRepository::default();
            repository
                .expect_query_user_search_filter_match_records_for_filter()
                .return_once(|_, _, _, _| {
                    Box::pin(async { Err(SdkError::construction_failure("test error")) })
                });
            let user_service = user::service::user_service::MockUserService::default();
            let service = UserSearchFilterServiceImpl::new(&repository, &user_service);

            let actual = service
                .view_search_filter_matches(&UserId::new(), &UserSearchFilterId::new(), &None, None)
                .await;
            assert!(actual.is_err());
        }
    }

    mod count_user_search_filter_matches_for_this_month {
        use crate::dynamodb::repository::MockUserSearchFilterDynamoDbRepository;
        use crate::service::user_search_filter_service::{
            UserSearchFilterError, UserSearchFilterService, UserSearchFilterServiceImpl,
        };
        use common::user_id::UserId;

        #[tokio::test]
        async fn should_return_count_when_matches_exist() {
            let mut repository = MockUserSearchFilterDynamoDbRepository::default();
            repository
                .expect_count_user_search_filter_match_records_for_between()
                .return_once(|_, _, _| Box::pin(async { Ok(7) }));
            let user_service = user::service::user_service::MockUserService::default();
            let service = UserSearchFilterServiceImpl::new(&repository, &user_service);

            let actual = service
                .count_user_search_filter_matches_for_this_month(&UserId::new())
                .await;
            assert!(actual.is_ok());
            assert_eq!(actual.unwrap(), 7);
        }

        #[tokio::test]
        async fn should_return_zero_when_no_matches() {
            let mut repository = MockUserSearchFilterDynamoDbRepository::default();
            repository
                .expect_count_user_search_filter_match_records_for_between()
                .return_once(|_, _, _| Box::pin(async { Ok(0) }));
            let user_service = user::service::user_service::MockUserService::default();
            let service = UserSearchFilterServiceImpl::new(&repository, &user_service);

            let actual = service
                .count_user_search_filter_matches_for_this_month(&UserId::new())
                .await;
            assert!(actual.is_ok());
            assert_eq!(actual.unwrap(), 0);
        }

        #[tokio::test]
        async fn should_propagate_sdk_error() {
            use aws_sdk_dynamodb::error::SdkError;
            let mut repository = MockUserSearchFilterDynamoDbRepository::default();
            repository
                .expect_count_user_search_filter_match_records_for_between()
                .return_once(|_, _, _| {
                    Box::pin(async { Err(SdkError::construction_failure("test error")) })
                });
            let user_service = user::service::user_service::MockUserService::default();
            let service = UserSearchFilterServiceImpl::new(&repository, &user_service);

            let actual = service
                .count_user_search_filter_matches_for_this_month(&UserId::new())
                .await;
            assert!(actual.is_err());
            match actual.unwrap_err() {
                UserSearchFilterError::SdkQueryError(_) => {}
                _ => panic!("expected SdkQueryError"),
            }
        }
    }
}

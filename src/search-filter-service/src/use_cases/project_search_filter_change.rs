use crate::ports::{
    SearchFilterIndex, SearchFilterIndexError, SearchFilterIndexReadError, SearchFilterIndexReader,
    SearchFilterProjectionWriteOutcome,
};
use common::error::boxed::{BoxError, box_error};
use common::user_search_filter_id::UserSearchFilterId;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchFilterProjectionOperation {
    Upsert,
    Delete,
}

impl SearchFilterProjectionOperation {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Upsert => "upsert",
            Self::Delete => "delete",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectSearchFilterChangeCommand {
    pub search_filter_id: UserSearchFilterId,
    pub source_version: i64,
    pub operation: SearchFilterProjectionOperation,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectSearchFilterChangeResult {
    pub outcome: SearchFilterProjectionWriteOutcome,
}

#[derive(Debug, thiserror::Error)]
pub enum ProjectSearchFilterChangeError {
    #[error("search filter projection source version must be positive")]
    InvalidSourceVersion,
    #[error("search filter projection delete version overflowed")]
    DeleteVersionOverflow,
    #[error("search filter projection read failed")]
    ReadFailed {
        #[source]
        source: BoxError,
    },
    #[error("search filter projection state is invalid")]
    InvalidPersistedState {
        #[source]
        source: BoxError,
    },
    #[error("search filter projection write failed")]
    WriteFailed {
        #[source]
        source: BoxError,
    },
}

#[async_trait::async_trait]
pub trait ProjectSearchFilterChangeUseCase: Send + Sync {
    async fn execute(
        &self,
        command: ProjectSearchFilterChangeCommand,
    ) -> Result<ProjectSearchFilterChangeResult, ProjectSearchFilterChangeError>;
}

/// Projects only authoritative SearchFilter state. FX selection belongs to
/// Product-event percolation, never to the saved-filter projection.
pub struct ProjectSearchFilterChangeHandler<R, I> {
    source: R,
    index: I,
}

impl<R, I> ProjectSearchFilterChangeHandler<R, I> {
    pub fn new(source: R, index: I) -> Self {
        Self { source, index }
    }
}

#[async_trait::async_trait]
impl<R, I> ProjectSearchFilterChangeUseCase for ProjectSearchFilterChangeHandler<R, I>
where
    R: SearchFilterIndexReader,
    I: SearchFilterIndex,
{
    #[tracing::instrument(
        name = "project_search_filter_change",
        skip_all,
        fields(
            search_filter_id = %command.search_filter_id,
            source_version = command.source_version,
            operation = command.operation.as_str(),
        )
    )]
    async fn execute(
        &self,
        command: ProjectSearchFilterChangeCommand,
    ) -> Result<ProjectSearchFilterChangeResult, ProjectSearchFilterChangeError> {
        if command.source_version <= 0 {
            return Err(ProjectSearchFilterChangeError::InvalidSourceVersion);
        }

        let outcome = match command.operation {
            SearchFilterProjectionOperation::Delete => self
                .index
                .delete(
                    command.search_filter_id,
                    tombstone_version(command.source_version)?,
                )
                .await
                .map_err(index_error)?,
            SearchFilterProjectionOperation::Upsert => match self
                .source
                .find_by_id(command.search_filter_id)
                .await
                .map_err(read_error)?
            {
                Some(projection) => self.index.upsert(&projection).await.map_err(index_error)?,
                None => self
                    .index
                    .delete(
                        command.search_filter_id,
                        tombstone_version(command.source_version)?,
                    )
                    .await
                    .map_err(index_error)?,
            },
        };

        Ok(ProjectSearchFilterChangeResult { outcome })
    }
}

fn tombstone_version(source_version: i64) -> Result<i64, ProjectSearchFilterChangeError> {
    source_version
        .checked_add(1)
        .ok_or(ProjectSearchFilterChangeError::DeleteVersionOverflow)
}

fn read_error(error: SearchFilterIndexReadError) -> ProjectSearchFilterChangeError {
    match error {
        SearchFilterIndexReadError::InvalidPersistedState { source } => {
            ProjectSearchFilterChangeError::InvalidPersistedState { source }
        }
        error => ProjectSearchFilterChangeError::ReadFailed {
            source: box_error(error),
        },
    }
}

fn index_error(error: SearchFilterIndexError) -> ProjectSearchFilterChangeError {
    ProjectSearchFilterChangeError::WriteFailed {
        source: box_error(error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ports::{SearchFilterIndexQuery, SearchFilterProjection, SearchFilterView};
    use common::pagination::cursor::CursoredResult;
    use common::resource_state::domain::ResourceState;
    use common::user_id::UserId;
    use common::user_search_filter_name::UserSearchFilterName;
    use localization::Language;
    use money::Currency;
    use product_core::product_search::ProductSearch;
    use std::sync::Mutex;
    use time::macros::datetime;

    #[derive(Default)]
    struct Source {
        projection: Mutex<Option<SearchFilterProjection>>,
    }

    #[async_trait::async_trait]
    impl SearchFilterIndexReader for Source {
        async fn find_by_id(
            &self,
            _search_filter_id: UserSearchFilterId,
        ) -> Result<Option<SearchFilterProjection>, SearchFilterIndexReadError> {
            self.projection
                .lock()
                .map_err(|_| SearchFilterIndexReadError::ReadFailed {
                    source: box_error(std::io::Error::other("test mutex poisoned")),
                })
                .map(|projection| projection.clone())
        }

        async fn list_after(
            &self,
            _after: Option<UserSearchFilterId>,
            _limit: usize,
        ) -> Result<Vec<SearchFilterProjection>, SearchFilterIndexReadError> {
            Ok(Vec::new())
        }
    }

    #[derive(Default)]
    struct Index {
        upserts: Mutex<Vec<SearchFilterProjection>>,
        deletes: Mutex<Vec<i64>>,
    }

    #[async_trait::async_trait]
    impl SearchFilterIndex for Index {
        async fn upsert(
            &self,
            projection: &SearchFilterProjection,
        ) -> Result<SearchFilterProjectionWriteOutcome, SearchFilterIndexError> {
            self.upserts
                .lock()
                .map_err(|_| SearchFilterIndexError::WriteFailed {
                    source: box_error(std::io::Error::other("test mutex poisoned")),
                })?
                .push(projection.clone());
            Ok(SearchFilterProjectionWriteOutcome::Applied)
        }

        async fn delete(
            &self,
            _id: UserSearchFilterId,
            source_version: i64,
        ) -> Result<SearchFilterProjectionWriteOutcome, SearchFilterIndexError> {
            self.deletes
                .lock()
                .map_err(|_| SearchFilterIndexError::DeleteFailed {
                    source: box_error(std::io::Error::other("test mutex poisoned")),
                })?
                .push(source_version);
            Ok(SearchFilterProjectionWriteOutcome::Applied)
        }

        async fn percolate(
            &self,
            _input: &product_service::ports::ProductPercolationInput,
        ) -> Result<Vec<SearchFilterView>, SearchFilterIndexError> {
            Ok(Vec::new())
        }

        async fn query(
            &self,
            _query: &SearchFilterIndexQuery,
        ) -> Result<CursoredResult<SearchFilterView, serde_json::Value>, SearchFilterIndexError>
        {
            Ok(CursoredResult::default())
        }
    }

    fn projection(version: i64) -> SearchFilterProjection {
        SearchFilterProjection {
            view: SearchFilterView {
                search_filter_id: UserSearchFilterId::new(),
                user_id: UserId::new(),
                name: UserSearchFilterName::from("daily"),
                notifications: true,
                state: ResourceState::Active,
                search: ProductSearch::new(Language::En, Currency::Usd),
                embedding: None,
                created: datetime!(2026-01-01 0:00 UTC),
                updated: datetime!(2026-01-01 0:00 UTC),
                last_hybrid_search_matched: datetime!(2026-01-01 0:00 UTC),
            },
            source_version: version,
        }
    }

    #[tokio::test]
    async fn should_project_authoritative_search_filter_state_without_fx_dependencies()
    -> Result<(), Box<dyn std::error::Error>> {
        let current = projection(4);
        let id = current.view.search_filter_id;
        let handler = ProjectSearchFilterChangeHandler::new(
            Source {
                projection: Mutex::new(Some(current.clone())),
            },
            Index::default(),
        );

        handler
            .execute(ProjectSearchFilterChangeCommand {
                search_filter_id: id,
                source_version: 2,
                operation: SearchFilterProjectionOperation::Upsert,
            })
            .await?;

        let upserts = handler
            .index
            .upserts
            .lock()
            .map_err(|_| std::io::Error::other("test mutex poisoned"))?
            .clone();
        assert_eq!(vec![current], upserts);
        Ok(())
    }

    #[tokio::test]
    async fn should_write_successor_delete_tombstone_for_deleted_source()
    -> Result<(), Box<dyn std::error::Error>> {
        let handler = ProjectSearchFilterChangeHandler::new(Source::default(), Index::default());

        handler
            .execute(ProjectSearchFilterChangeCommand {
                search_filter_id: UserSearchFilterId::new(),
                source_version: 7,
                operation: SearchFilterProjectionOperation::Delete,
            })
            .await?;

        let deletes = handler
            .index
            .deletes
            .lock()
            .map_err(|_| std::io::Error::other("test mutex poisoned"))?
            .clone();
        assert_eq!(vec![8], deletes);
        Ok(())
    }

    #[tokio::test]
    async fn should_reject_invalid_source_versions() {
        let handler = ProjectSearchFilterChangeHandler::new(Source::default(), Index::default());

        let result = handler
            .execute(ProjectSearchFilterChangeCommand {
                search_filter_id: UserSearchFilterId::new(),
                source_version: 0,
                operation: SearchFilterProjectionOperation::Upsert,
            })
            .await;

        assert!(matches!(
            result,
            Err(ProjectSearchFilterChangeError::InvalidSourceVersion)
        ));
    }
}

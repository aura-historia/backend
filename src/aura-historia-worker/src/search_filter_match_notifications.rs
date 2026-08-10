use crate::{
    InMemoryQueueReceiver,
    cdc::{DomainJob, DomainJobPayload},
    retry::{InMemoryDeadLetterQueue, RetryConfig, run_with_retry},
};
use common::{
    error::boxed::{BoxError, box_error},
    postgres::SqlxUnitOfWork,
    product_id::ProductId,
    transaction::{Transaction, UnitOfWork},
    user_id::UserId,
    user_search_filter_id::UserSearchFilterId,
};
use product_postgres::SqlxProductSearchFilterMatchSourceReaderFactory;
use product_service::ports::{
    ProductSearchFilterMatchSourceReadError, ProductSearchFilterMatchSourceReader,
    ProductSearchFilterMatchSourceReaderFactory,
};
use search_filter_postgres::SqlxSearchFilterMatchNotificationSourceReaderFactory;
use search_filter_service::{
    ports::{
        SearchFilterMatchNotificationSourceReadError, SearchFilterMatchNotificationSourceReader,
        SearchFilterMatchNotificationSourceReaderFactory,
    },
    use_cases::{
        GenerateSearchFilterMatchNotificationCommand, GenerateSearchFilterMatchNotificationUseCase,
    },
};
use std::sync::Arc;
use tracing::{error, info};

pub async fn consume_search_filter_match_notification_queue(
    mut receiver: InMemoryQueueReceiver<DomainJob>,
    handler: Arc<dyn GenerateSearchFilterMatchNotificationUseCase>,
    match_unit_of_work: SqlxUnitOfWork,
    match_source_reader_factory: SqlxSearchFilterMatchNotificationSourceReaderFactory,
    product_unit_of_work: SqlxUnitOfWork,
    product_source_reader_factory: SqlxProductSearchFilterMatchSourceReaderFactory,
) {
    let dead_letters = InMemoryDeadLetterQueue::new();

    while let Some(job) = receiver.recv().await {
        let idempotency_key = job.idempotency_key.as_str().to_owned();
        let ordering_key = job.ordering_key.as_str().to_owned();
        let handler_for_retry = Arc::clone(&handler);
        let match_unit_of_work_for_retry = match_unit_of_work.clone();
        let match_source_reader_factory_for_retry = match_source_reader_factory.clone();
        let product_unit_of_work_for_retry = product_unit_of_work.clone();
        let product_source_reader_factory_for_retry = product_source_reader_factory;
        let result = run_with_retry(job, RetryConfig::default(), &dead_letters, move |job| {
            let handler = Arc::clone(&handler_for_retry);
            let match_unit_of_work = match_unit_of_work_for_retry.clone();
            let match_source_reader_factory = match_source_reader_factory_for_retry.clone();
            let product_unit_of_work = product_unit_of_work_for_retry.clone();
            let product_source_reader_factory = product_source_reader_factory_for_retry;
            async move {
                generate_search_filter_match_notification(
                    handler,
                    match_unit_of_work,
                    match_source_reader_factory,
                    product_unit_of_work,
                    product_source_reader_factory,
                    job,
                )
                .await
            }
        })
        .await;

        match result {
            Ok(()) => info!(
                job_type = "search_filter_match_notification",
                %idempotency_key,
                %ordering_key,
                outcome = "applied",
                "search filter match notification job completed"
            ),
            Err(error) => error!(
                job_type = "search_filter_match_notification",
                %idempotency_key,
                %ordering_key,
                error = %error,
                outcome = "dead_lettered_in_memory",
                "search filter match notification job failed"
            ),
        }
    }
}

async fn generate_search_filter_match_notification(
    handler: Arc<dyn GenerateSearchFilterMatchNotificationUseCase>,
    match_unit_of_work: SqlxUnitOfWork,
    match_source_reader_factory: SqlxSearchFilterMatchNotificationSourceReaderFactory,
    product_unit_of_work: SqlxUnitOfWork,
    product_source_reader_factory: SqlxProductSearchFilterMatchSourceReaderFactory,
    job: DomainJob,
) -> Result<(), SearchFilterMatchNotificationWorkerError> {
    let DomainJobPayload::SearchFilterMatchCreated(change) = job.payload else {
        return Err(SearchFilterMatchNotificationWorkerError::UnexpectedJobPayload);
    };
    let user_id = UserId::try_from(change.user_id.as_str()).map_err(|source| {
        SearchFilterMatchNotificationWorkerError::InvalidUserId {
            source: box_error(source),
        }
    })?;
    let search_filter_id = UserSearchFilterId::try_from(change.user_search_filter_id.as_str())
        .map_err(
            |source| SearchFilterMatchNotificationWorkerError::InvalidSearchFilterId {
                source: box_error(source),
            },
        )?;
    let product_id = ProductId::try_from(change.product_id.as_str()).map_err(|source| {
        SearchFilterMatchNotificationWorkerError::InvalidProductId {
            source: box_error(source),
        }
    })?;

    let mut match_tx = match_unit_of_work.begin().await.map_err(|source| {
        SearchFilterMatchNotificationWorkerError::BeginMatchReadTransaction {
            source: box_error(source),
        }
    })?;
    let match_source = match_source_reader_factory
        .in_transaction(&mut match_tx)
        .find_source(user_id, search_filter_id, product_id)
        .await
        .map_err(match_source_read_error)?
        .ok_or(
            SearchFilterMatchNotificationWorkerError::MatchSourceNotFound {
                user_id,
                search_filter_id,
                product_id,
            },
        )?;
    match_tx.commit().await.map_err(|source| {
        SearchFilterMatchNotificationWorkerError::CommitMatchReadTransaction {
            source: box_error(source),
        }
    })?;

    let mut product_tx = product_unit_of_work.begin().await.map_err(|source| {
        SearchFilterMatchNotificationWorkerError::BeginProductReadTransaction {
            source: box_error(source),
        }
    })?;
    let product = product_source_reader_factory
        .in_transaction(&mut product_tx)
        .find_source(match_source.origin_event_id, match_source.product_id)
        .await
        .map_err(product_source_read_error)?
        .ok_or(
            SearchFilterMatchNotificationWorkerError::ProductSourceNotFound {
                origin_event_id: match_source.origin_event_id,
                product_id: match_source.product_id,
            },
        )?;
    product_tx.commit().await.map_err(|source| {
        SearchFilterMatchNotificationWorkerError::CommitProductReadTransaction {
            source: box_error(source),
        }
    })?;

    handler
        .execute(GenerateSearchFilterMatchNotificationCommand {
            match_source,
            product,
        })
        .await
        .map(|_| ())
        .map_err(
            |source| SearchFilterMatchNotificationWorkerError::Generate {
                source: box_error(source),
            },
        )
}

fn match_source_read_error(
    error: SearchFilterMatchNotificationSourceReadError,
) -> SearchFilterMatchNotificationWorkerError {
    match error {
        SearchFilterMatchNotificationSourceReadError::InvalidPersistedState { source } => {
            SearchFilterMatchNotificationWorkerError::MatchSourceStateInvalid { source }
        }
        error => SearchFilterMatchNotificationWorkerError::ReadMatchSource {
            source: box_error(error),
        },
    }
}

fn product_source_read_error(
    error: ProductSearchFilterMatchSourceReadError,
) -> SearchFilterMatchNotificationWorkerError {
    match error {
        ProductSearchFilterMatchSourceReadError::InvalidPersistedState { source } => {
            SearchFilterMatchNotificationWorkerError::ProductSourceStateInvalid { source }
        }
        error => SearchFilterMatchNotificationWorkerError::ReadProductSource {
            source: box_error(error),
        },
    }
}

#[derive(Debug, thiserror::Error)]
enum SearchFilterMatchNotificationWorkerError {
    #[error("search filter match notification queue received an unexpected job payload")]
    UnexpectedJobPayload,
    #[error("search filter match notification job has an invalid user id")]
    InvalidUserId {
        #[source]
        source: BoxError,
    },
    #[error("search filter match notification job has an invalid search filter id")]
    InvalidSearchFilterId {
        #[source]
        source: BoxError,
    },
    #[error("search filter match notification job has an invalid product id")]
    InvalidProductId {
        #[source]
        source: BoxError,
    },
    #[error("failed to begin search filter match source read transaction")]
    BeginMatchReadTransaction {
        #[source]
        source: BoxError,
    },
    #[error("search filter match notification source read failed")]
    ReadMatchSource {
        #[source]
        source: BoxError,
    },
    #[error("search filter match notification source persisted state is invalid")]
    MatchSourceStateInvalid {
        #[source]
        source: BoxError,
    },
    #[error("search filter match notification source was not found")]
    MatchSourceNotFound {
        user_id: UserId,
        search_filter_id: UserSearchFilterId,
        product_id: ProductId,
    },
    #[error("failed to commit search filter match source read transaction")]
    CommitMatchReadTransaction {
        #[source]
        source: BoxError,
    },
    #[error("failed to begin product source read transaction")]
    BeginProductReadTransaction {
        #[source]
        source: BoxError,
    },
    #[error("product source read failed")]
    ReadProductSource {
        #[source]
        source: BoxError,
    },
    #[error("product source persisted state is invalid")]
    ProductSourceStateInvalid {
        #[source]
        source: BoxError,
    },
    #[error("product source was not found")]
    ProductSourceNotFound {
        origin_event_id: common::event_id::EventId,
        product_id: ProductId,
    },
    #[error("failed to commit product source read transaction")]
    CommitProductReadTransaction {
        #[source]
        source: BoxError,
    },
    #[error("search filter match notification generation failed")]
    Generate {
        #[source]
        source: BoxError,
    },
}

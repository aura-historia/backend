use crate::ports::{
    AuctionDirectoryReadError, AuctionDirectoryReader, ListAuctionsDirectoryRequest,
    ListAuctionsDirectoryResult,
};
use application::{error::BoxError, operation_context::OperationContext};

const MAX_DIRECTORY_SIZE: u64 = 100;

pub type ListAuctionsRequest = ListAuctionsDirectoryRequest;
pub type ListAuctionsResult = ListAuctionsDirectoryResult;

#[derive(Debug, thiserror::Error)]
pub enum ListAuctionsError {
    #[error("Auction schedule instant filter requires both range bounds")]
    IncompleteScheduleInstantRange,
    #[error("Auction schedule instant filter must have an increasing range")]
    InvalidScheduleInstantRange,
    #[error("Auction directory scheduled sort requires a matching time role")]
    ScheduledSortRoleMismatch,
    #[error("Auction directory cursor belongs to another filter or sort scope")]
    CursorScopeMismatch,
    #[error("Auction directory is temporarily unavailable")]
    TemporarilyUnavailable {
        #[source]
        source: BoxError,
    },
    #[error("Auction directory read model is invalid")]
    InvalidReadModel {
        #[source]
        source: BoxError,
    },
}

#[async_trait::async_trait]
pub trait ListAuctionsUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        request: ListAuctionsRequest,
    ) -> Result<ListAuctionsResult, ListAuctionsError>;
}

pub struct ListAuctionsHandler<R> {
    directory: R,
}

impl<R> ListAuctionsHandler<R> {
    pub fn new(directory: R) -> Self {
        Self { directory }
    }
}

#[async_trait::async_trait]
impl<R> ListAuctionsUseCase for ListAuctionsHandler<R>
where
    R: AuctionDirectoryReader,
{
    #[tracing::instrument(
        name = "list_auctions",
        skip_all,
        fields(
            principal_type = context.principal.kind(),
            actor_id = tracing::field::Empty,
            request_id = %context.request_id,
            correlation_id = %context.correlation_id,
        )
    )]
    async fn execute(
        &self,
        context: &OperationContext,
        request: ListAuctionsRequest,
    ) -> Result<ListAuctionsResult, ListAuctionsError> {
        if let Some(actor_id) = context.principal.actor_id() {
            tracing::Span::current().record("actor_id", tracing::field::display(actor_id));
        }
        let request = validate_and_bound(request)?;
        self.directory.list(&request).await.map_err(Into::into)
    }
}

fn validate_and_bound(
    mut request: ListAuctionsRequest,
) -> Result<ListAuctionsRequest, ListAuctionsError> {
    if request.sort.is_some_and(|role| {
        request
            .schedule
            .as_ref()
            .is_some_and(|schedule| schedule.role != role)
    }) {
        return Err(ListAuctionsError::ScheduledSortRoleMismatch);
    }
    if let Some(schedule) = request.schedule.as_ref() {
        let (Some(min), Some(max)) = (schedule.range.min, schedule.range.max) else {
            return Err(ListAuctionsError::IncompleteScheduleInstantRange);
        };
        if min >= max {
            return Err(ListAuctionsError::InvalidScheduleInstantRange);
        }
    }
    let scope = request.scope();
    if request
        .cursor
        .as_ref()
        .and_then(|cursor| cursor.search_after.as_ref())
        .is_some_and(|cursor| cursor.scope != scope)
    {
        return Err(ListAuctionsError::CursorScopeMismatch);
    }
    if let Some(cursor) = request.cursor.as_mut() {
        cursor.size = cursor.size.clamp(1, MAX_DIRECTORY_SIZE);
    }
    Ok(request)
}

impl From<AuctionDirectoryReadError> for ListAuctionsError {
    fn from(error: AuctionDirectoryReadError) -> Self {
        match error {
            AuctionDirectoryReadError::QueryFailed { source } => {
                Self::TemporarilyUnavailable { source }
            }
            AuctionDirectoryReadError::InvalidReadModel { source } => {
                Self::InvalidReadModel { source }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ports::{
        AuctionDirectoryCursor, AuctionDirectoryScope, AuctionInstantScheduleFilter,
    };
    use application::pagination::Cursor;
    use auction_core::AuctionId;
    use auction_core::AuctionSchedulePoint;
    use domain_primitives::query::range_query::RangeQuery;
    use time::macros::datetime;

    #[test]
    fn should_reject_a_schedule_filter_without_both_exact_instant_bounds() {
        let request = ListAuctionsRequest {
            schedule: Some(AuctionInstantScheduleFilter {
                role: AuctionSchedulePoint::ScheduledEnd,
                range: RangeQuery {
                    min: Some(datetime!(2026-01-01 00:00 UTC)),
                    max: None,
                },
            }),
            ..Default::default()
        };

        assert!(matches!(
            validate_and_bound(request),
            Err(ListAuctionsError::IncompleteScheduleInstantRange)
        ));
    }

    #[test]
    fn should_reject_a_non_increasing_schedule_filter_range() {
        let instant = datetime!(2026-01-01 00:00 UTC);
        let request = ListAuctionsRequest {
            schedule: Some(AuctionInstantScheduleFilter {
                role: AuctionSchedulePoint::ScheduledEnd,
                range: RangeQuery {
                    min: Some(instant),
                    max: Some(instant),
                },
            }),
            ..Default::default()
        };

        assert!(matches!(
            validate_and_bound(request),
            Err(ListAuctionsError::InvalidScheduleInstantRange)
        ));
    }

    #[test]
    fn should_reject_a_directory_cursor_for_another_filter_scope() {
        let request = ListAuctionsRequest {
            format: Some(auction_core::AuctionFormat::Timed),
            cursor: Some(Cursor {
                size: 21,
                search_after: Some(AuctionDirectoryCursor {
                    created: datetime!(2026-01-01 00:00 UTC),
                    auction_id: AuctionId::new(),
                    scheduled: None,
                    scope: AuctionDirectoryScope {
                        listing_source_id: None,
                        format: Some(auction_core::AuctionFormat::Live),
                        reported_status: None,
                        schedule: None,
                        sort: None,
                        order: None,
                    },
                }),
            }),
            ..Default::default()
        };

        assert!(matches!(
            validate_and_bound(request),
            Err(ListAuctionsError::CursorScopeMismatch)
        ));
    }

    #[test]
    fn should_reject_a_cursor_for_another_sort_or_order() {
        let scheduled = AuctionSchedulePoint::LiveStarts;
        let cursor = AuctionDirectoryCursor {
            created: datetime!(2026-01-01 00:00 UTC),
            scheduled: None,
            auction_id: AuctionId::new(),
            scope: AuctionDirectoryScope {
                listing_source_id: None,
                format: None,
                reported_status: None,
                schedule: None,
                sort: Some(scheduled),
                order: Some(domain_primitives::sort::SortOrder::Asc),
            },
        };
        for (sort, order) in [
            (None, None),
            (
                Some(scheduled),
                Some(domain_primitives::sort::SortOrder::Desc),
            ),
            (Some(AuctionSchedulePoint::ScheduledEnd), None),
        ] {
            let request = ListAuctionsRequest {
                sort,
                order,
                cursor: Some(Cursor {
                    size: 21,
                    search_after: Some(cursor.clone()),
                }),
                ..Default::default()
            };
            assert!(matches!(
                validate_and_bound(request),
                Err(ListAuctionsError::CursorScopeMismatch)
            ));
        }
    }

    #[test]
    fn should_default_to_created_desc_and_scheduled_asc() {
        let created = ListAuctionsRequest::default();
        assert_eq!(domain_primitives::sort::SortOrder::Desc, created.order());
        assert_eq!(None, created.scope().order);
        let scheduled = ListAuctionsRequest {
            sort: Some(AuctionSchedulePoint::LiveStarts),
            ..Default::default()
        };
        assert_eq!(domain_primitives::sort::SortOrder::Asc, scheduled.order());
        assert_eq!(Some(scheduled.order()), scheduled.scope().order);
    }

    #[test]
    fn should_accept_legacy_cursor_with_explicit_created_desc() {
        let request = ListAuctionsRequest {
            order: Some(domain_primitives::sort::SortOrder::Desc),
            cursor: Some(Cursor {
                size: 21,
                search_after: Some(AuctionDirectoryCursor {
                    created: datetime!(2026-01-01 00:00 UTC),
                    auction_id: AuctionId::new(),
                    scheduled: None,
                    scope: ListAuctionsRequest::default().scope(),
                }),
            }),
            ..Default::default()
        };
        assert!(validate_and_bound(request).is_ok());
    }

    #[test]
    fn should_reject_conflicting_scheduled_sort_and_filter_roles() {
        let request = ListAuctionsRequest {
            sort: Some(AuctionSchedulePoint::LiveStarts),
            schedule: Some(AuctionInstantScheduleFilter {
                role: AuctionSchedulePoint::ScheduledEnd,
                range: RangeQuery {
                    min: Some(datetime!(2026-01-01 00:00 UTC)),
                    max: Some(datetime!(2026-01-02 00:00 UTC)),
                },
            }),
            ..Default::default()
        };
        assert!(matches!(
            validate_and_bound(request),
            Err(ListAuctionsError::ScheduledSortRoleMismatch)
        ));
    }

    #[test]
    fn should_clamp_the_directory_cursor_to_the_supported_size() {
        let request = ListAuctionsRequest {
            cursor: Some(application::pagination::Cursor {
                size: 101,
                search_after: None,
            }),
            ..Default::default()
        };

        let result = validate_and_bound(request);
        assert!(
            matches!(result, Ok(value) if value.cursor.as_ref().map(|cursor| cursor.size) == Some(100))
        );
    }
}

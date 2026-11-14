use crate::use_cases::queries::list_admin_content_notices::{
    ListAdminContentNoticesRequest, ListAdminContentNoticesResult,
};
use application::error::BoxError;

#[derive(Debug, thiserror::Error)]
pub enum ContentNoticeAdminSearchReadError {
    #[error("temporary content notice search failure")]
    TemporarilyUnavailable {
        #[source]
        source: BoxError,
    },
    #[error("invalid content notice search read model")]
    InvalidReadModel {
        #[source]
        source: BoxError,
    },
    #[error("internal content notice search failure")]
    Internal {
        #[source]
        source: BoxError,
    },
}

/// Reads one page of the admin review queue. Implementations clamp the page size to 1–100,
/// continue strictly after the request's cursor position and return a cursor only when another
/// page exists.
#[async_trait::async_trait]
pub trait ContentNoticeAdminSearchReader: Send {
    async fn search(
        &mut self,
        request: &ListAdminContentNoticesRequest,
    ) -> Result<ListAdminContentNoticesResult, ContentNoticeAdminSearchReadError>;
}

pub trait ContentNoticeAdminSearchReaderFactory<Tx>: Send + Sync {
    fn in_transaction<'tx>(&'tx self, tx: &'tx mut Tx)
    -> impl ContentNoticeAdminSearchReader + 'tx;
}

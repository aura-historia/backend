use crate::use_cases::queries::get_admin_content_notice::AdminContentNoticeDetails;
use application::error::BoxError;
use content_notice_core::content_notice_id::ContentNoticeId;

#[derive(Debug, thiserror::Error)]
pub enum ContentNoticeDetailsReadError {
    #[error("temporary content notice details read failure")]
    TemporarilyUnavailable {
        #[source]
        source: BoxError,
    },
    #[error("invalid content notice details read model")]
    InvalidReadModel {
        #[source]
        source: BoxError,
    },
}

/// Reads one notice for the admin detail view together with the current state of its target, in
/// a small fixed number of queries.
#[async_trait::async_trait]
pub trait ContentNoticeDetailsReader: Send {
    async fn find_by_id(
        &mut self,
        content_notice_id: ContentNoticeId,
    ) -> Result<Option<AdminContentNoticeDetails>, ContentNoticeDetailsReadError>;
}

pub trait ContentNoticeDetailsReaderFactory<Tx>: Send + Sync {
    fn in_transaction<'tx>(&'tx self, tx: &'tx mut Tx) -> impl ContentNoticeDetailsReader + 'tx;
}

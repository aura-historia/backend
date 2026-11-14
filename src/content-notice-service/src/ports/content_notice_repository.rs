use application::error::BoxError;
use content_notice_core::{content_notice::ContentNotice, content_notice_id::ContentNoticeId};
use domain_primitives::versioned::Versioned;

domain_primitives::version_newtype!(ContentNoticeStorageVersion, no_serde);
pub type VersionedContentNotice = Versioned<ContentNotice, ContentNoticeStorageVersion>;

/// SHA-256 digest that identifies a submission for the submission rules. The submission use
/// case defines what it covers; storage only keeps it beside the notice.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct ContentNoticeSubmissionFingerprint([u8; 32]);

impl ContentNoticeSubmissionFingerprint {
    pub const fn from_sha256(digest: [u8; 32]) -> Self {
        Self(digest)
    }

    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

impl std::fmt::Debug for ContentNoticeSubmissionFingerprint {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_tuple("ContentNoticeSubmissionFingerprint")
            .field(&"<redacted>")
            .finish()
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ContentNoticeRepositoryError {
    #[error("temporary content notice persistence failure")]
    TemporarilyUnavailable {
        #[source]
        source: BoxError,
    },
    #[error("invalid persisted content notice state")]
    InvalidPersistedState {
        #[source]
        source: BoxError,
    },
    #[error("internal content notice persistence failure")]
    Internal {
        #[source]
        source: BoxError,
    },
}

#[async_trait::async_trait]
pub trait ContentNoticeRepository: Send {
    async fn insert(
        &mut self,
        notice: &ContentNotice,
        fingerprint: ContentNoticeSubmissionFingerprint,
    ) -> Result<VersionedContentNotice, ContentNoticeRepositoryError>;

    async fn find_by_id(
        &mut self,
        id: ContentNoticeId,
    ) -> Result<Option<VersionedContentNotice>, ContentNoticeRepositoryError>;
}

pub trait ContentNoticeRepositoryFactory<Tx>: Send + Sync {
    fn in_transaction<'tx>(&'tx self, tx: &'tx mut Tx) -> impl ContentNoticeRepository + 'tx;
}

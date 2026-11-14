use application::error::BoxError;
use localization::Language;
use notification_core::{
    external_email::{ExternalEmailContent, ExternalEmailOrigin, ExternalEmailRecipient},
    external_email_message_id::ExternalEmailMessageId,
};

/// A transactional email to an address that does not belong to a user account.
/// `recipient` and `content` are personal data: never log them.
#[derive(Debug, Clone, PartialEq)]
pub struct ExternalEmailRequest {
    pub recipient: ExternalEmailRecipient,
    /// Rendered in this language when a template exists for it, otherwise in English.
    pub language: Language,
    pub content: ExternalEmailContent,
    pub origin: ExternalEmailOrigin,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExternalEmailEnqueueOutcome {
    Enqueued(ExternalEmailMessageId),
    /// The origin already enqueued this template kind; nothing was written.
    AlreadyEnqueued(ExternalEmailMessageId),
}

impl ExternalEmailEnqueueOutcome {
    pub const fn external_email_message_id(self) -> ExternalEmailMessageId {
        match self {
            Self::Enqueued(id) | Self::AlreadyEnqueued(id) => id,
        }
    }
}

/// The result of erasing an origin's messages.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ExternalEmailErasure {
    /// Messages erased together with their finished (delivered or failed) deliveries.
    pub erased: u64,
    /// Messages kept because their delivery is still pending or in progress.
    pub in_flight: u64,
}

#[derive(Debug, thiserror::Error)]
pub enum ExternalEmailOutboxError {
    #[error("external email outbox operation failed")]
    OperationFailed {
        #[source]
        source: BoxError,
    },
    #[error("persisted external email state is invalid")]
    InvalidPersistedState {
        #[source]
        source: BoxError,
    },
}

/// Durable intent to send external emails, bound to the producer's transaction: the
/// message and its delivery commit or roll back together with the producer's business
/// state, and the notification delivery pipeline sends it after commit.
#[async_trait::async_trait]
pub trait ExternalEmailOutbox: Send {
    /// Inserts the message and its pending delivery. An origin that already enqueued the
    /// same template kind is an idempotent no-op that returns the existing message.
    async fn enqueue(
        &mut self,
        request: ExternalEmailRequest,
    ) -> Result<ExternalEmailEnqueueOutcome, ExternalEmailOutboxError>;

    /// Erases the messages of `origin` whose delivery has finished, together with those
    /// deliveries. Messages still pending or in progress are kept and counted, so erasure
    /// never races a send or strands a delivery job; erase them again once they finish.
    /// Origins call this to apply their own retention of the personal data in messages.
    async fn erase_origin(
        &mut self,
        origin: ExternalEmailOrigin,
    ) -> Result<ExternalEmailErasure, ExternalEmailOutboxError>;
}

pub trait ExternalEmailOutboxFactory<Tx>: Send + Sync {
    fn in_transaction<'tx>(&'tx self, tx: &'tx mut Tx) -> impl ExternalEmailOutbox + 'tx;
}

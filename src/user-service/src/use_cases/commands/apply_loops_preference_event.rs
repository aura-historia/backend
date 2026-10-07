use crate::ports::{
    ConsentUser, LoopsPreferenceFence, LoopsWebhookReceiptDisposition as Disposition,
    LoopsWebhookReceiptError, LoopsWebhookReceiptInput, LoopsWebhookReceiptLookup,
    LoopsWebhookReceiptWriteOutcome, LoopsWebhookReceipts, LoopsWebhookReceiptsFactory,
    MarketingConsentIntentError, MarketingConsentIntents, MarketingConsentIntentsFactory,
    MarketingEmailConsentWriter, MarketingEmailSubscriptionState, NewsletterWebhookEventKind,
    NewsletterWebhookMailingListId, NewsletterWebhookVerification,
    NewsletterWebhookVerificationOutcome,
};
use crate::use_cases::commands::coordinate_marketing_consent::{
    CoordinateMarketingConsentError, MarketingConsentCoordinator,
};
use application::transaction::{Transaction, UnitOfWork};
use serde_email::Email;
use time::{Duration, OffsetDateTime};
use user_core::user_id::UserId;

const WEBHOOK_RECEIPT_RETENTION: Duration = Duration::days(35);

/// Authenticated verifier output and the configured single marketing-purpose list.
/// The transport must verify the signature before calling this use case.
pub struct ApplyLoopsPreferenceEventCommand {
    pub verification: NewsletterWebhookVerification,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ApplyLoopsPreferenceEventOutcome {
    /// The provider observation and all local effects committed atomically.
    CommittedApplication(Disposition),
    /// The same provider delivery ID and exact raw-body digest were already committed.
    Duplicate,
    /// The event was safely ignored and a receipt committed.
    Ignored(Disposition),
    /// A delivery ID was reused with different authenticated body bytes.
    Conflict,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ApplyLoopsPreferenceEventError {
    #[error("Loops preference event processing must be retried")]
    Retryable,
    #[error("verified Loops event contains an invalid timestamp")]
    InvalidEventTime,
}

#[async_trait::async_trait]
pub trait ApplyLoopsPreferenceEventUseCase: Send + Sync {
    async fn execute(
        &self,
        command: ApplyLoopsPreferenceEventCommand,
    ) -> Result<ApplyLoopsPreferenceEventOutcome, ApplyLoopsPreferenceEventError>;
}

pub struct ApplyLoopsPreferenceEventHandler<U, I, R, P> {
    unit_of_work: U,
    intents: I,
    receipts: R,
    provider: P,
    target_list_id: NewsletterWebhookMailingListId,
}

impl<U, I, R, P> ApplyLoopsPreferenceEventHandler<U, I, R, P> {
    pub fn new(
        unit_of_work: U,
        intents: I,
        receipts: R,
        provider: P,
        target_list_id: NewsletterWebhookMailingListId,
    ) -> Self {
        Self {
            unit_of_work,
            intents,
            receipts,
            provider,
            target_list_id,
        }
    }
}

#[async_trait::async_trait]
impl<U, I, R, P> ApplyLoopsPreferenceEventUseCase for ApplyLoopsPreferenceEventHandler<U, I, R, P>
where
    U: UnitOfWork,
    I: MarketingConsentIntentsFactory<U::Tx>,
    R: LoopsWebhookReceiptsFactory<U::Tx>,
    P: MarketingEmailConsentWriter,
{
    async fn execute(
        &self,
        command: ApplyLoopsPreferenceEventCommand,
    ) -> Result<ApplyLoopsPreferenceEventOutcome, ApplyLoopsPreferenceEventError> {
        let processed_at = OffsetDateTime::now_utc();
        match command.verification.outcome {
            NewsletterWebhookVerificationOutcome::Ignored(event) => {
                let event_time = event_time(event.event_time_unix_seconds)?;
                let receipt = LoopsWebhookReceiptInput {
                    delivery_id: event.delivery_id.as_str().to_owned(),
                    raw_body_sha256: *command.verification.raw_body_sha256.as_bytes(),
                    event_name: event.provider_event_name.as_str().to_owned(),
                    event_time,
                    provider_contact_id: None,
                    email: None,
                    mailing_list_id: None,
                    disposition: Disposition::IgnoredUnsupportedEvent,
                    processed_at,
                    expires_at: processed_at + WEBHOOK_RECEIPT_RETENTION,
                };
                self.commit_ignored_receipt(receipt).await
            }
            NewsletterWebhookVerificationOutcome::Verified(event) => {
                let event_time = event_time(event.event_time_unix_seconds)?;
                let delivery_id = event.delivery_id.as_str().to_owned();
                let event_name = event.provider_event_name.as_str().to_owned();
                let raw_body_sha256 = *command.verification.raw_body_sha256.as_bytes();
                let email_text = event.email.as_str().to_owned();
                let contact_id = event.provider_contact_id.as_str().to_owned();
                let list_id = event
                    .mailing_list_id
                    .as_ref()
                    .map(|id| id.as_str().to_owned());
                let Some(email) = exact_email(&email_text) else {
                    return self
                        .commit_ignored_receipt(LoopsWebhookReceiptInput {
                            delivery_id,
                            raw_body_sha256,
                            event_name,
                            event_time,
                            provider_contact_id: Some(contact_id),
                            email: Some(email_text),
                            mailing_list_id: list_id,
                            disposition: Disposition::IgnoredInvalidRecipient,
                            processed_at,
                            expires_at: processed_at + WEBHOOK_RECEIPT_RETENTION,
                        })
                        .await;
                };
                if event_time > processed_at + Duration::minutes(1)
                    || (event.kind == NewsletterWebhookEventKind::EmailResubscribed
                        && event_time > processed_at)
                {
                    return self
                        .commit_ignored_receipt(LoopsWebhookReceiptInput {
                            delivery_id,
                            raw_body_sha256,
                            event_name,
                            event_time,
                            provider_contact_id: Some(contact_id),
                            email: Some(email_text),
                            mailing_list_id: list_id,
                            disposition: Disposition::IgnoredUntrustworthyTime,
                            processed_at,
                            expires_at: processed_at + WEBHOOK_RECEIPT_RETENTION,
                        })
                        .await;
                }

                let kind = event.kind;
                let event = VerifiedPreferenceEvent {
                    delivery_id,
                    raw_body_sha256,
                    event_name,
                    event_time,
                    email_text,
                    email,
                    contact_id,
                    mailing_list_id: list_id,
                    processed_at,
                };
                if kind == NewsletterWebhookEventKind::EmailResubscribed {
                    self.apply_resubscription(event).await
                } else {
                    self.apply_non_grant_event(event, kind).await
                }
            }
        }
    }
}

impl<U, I, R, P> ApplyLoopsPreferenceEventHandler<U, I, R, P>
where
    U: UnitOfWork,
    I: MarketingConsentIntentsFactory<U::Tx>,
    R: LoopsWebhookReceiptsFactory<U::Tx>,
    P: MarketingEmailConsentWriter,
{
    async fn commit_ignored_receipt(
        &self,
        receipt: LoopsWebhookReceiptInput,
    ) -> Result<ApplyLoopsPreferenceEventOutcome, ApplyLoopsPreferenceEventError> {
        let disposition = receipt.disposition;
        let mut tx = self
            .unit_of_work
            .begin()
            .await
            .map_err(|_| ApplyLoopsPreferenceEventError::Retryable)?;
        let write = self
            .receipts
            .in_transaction(&mut tx)
            .insert_receipt(receipt)
            .await
            .map_err(map_receipt_error)?;
        match write {
            LoopsWebhookReceiptWriteOutcome::Inserted => {
                tx.commit()
                    .await
                    .map_err(|_| ApplyLoopsPreferenceEventError::Retryable)?;
                Ok(ApplyLoopsPreferenceEventOutcome::Ignored(disposition))
            }
            LoopsWebhookReceiptWriteOutcome::ExistingSameDigest => {
                Ok(ApplyLoopsPreferenceEventOutcome::Duplicate)
            }
            LoopsWebhookReceiptWriteOutcome::ExistingDifferentDigest => {
                Ok(ApplyLoopsPreferenceEventOutcome::Conflict)
            }
        }
    }

    async fn lookup_delivery(
        &self,
        tx: &mut U::Tx,
        delivery_id: &str,
        raw_body_sha256: &[u8; 32],
    ) -> Result<Option<ApplyLoopsPreferenceEventOutcome>, ApplyLoopsPreferenceEventError> {
        let existing = self
            .receipts
            .in_transaction(tx)
            .find_by_delivery_id(delivery_id)
            .await
            .map_err(map_receipt_error)?;
        Ok(existing.map(|existing| match_digest(existing, raw_body_sha256)))
    }

    async fn lock_recipient(
        &self,
        tx: &mut U::Tx,
        email: &Email,
    ) -> Result<(), ApplyLoopsPreferenceEventError> {
        self.intents
            .in_transaction(tx)
            .lock_recipient(email)
            .await
            .map_err(map_intent_error)
    }

    async fn find_fence(
        &self,
        tx: &mut U::Tx,
        email: &Email,
    ) -> Result<Option<LoopsPreferenceFence>, ApplyLoopsPreferenceEventError> {
        self.receipts
            .in_transaction(tx)
            .find_preference_fence(email)
            .await
            .map_err(map_receipt_error)
    }

    async fn insert_and_commit(
        &self,
        mut tx: U::Tx,
        receipt: LoopsWebhookReceiptInput,
        applied: bool,
    ) -> Result<ApplyLoopsPreferenceEventOutcome, ApplyLoopsPreferenceEventError> {
        let disposition = receipt.disposition;
        let write = self
            .receipts
            .in_transaction(&mut tx)
            .insert_receipt(receipt)
            .await
            .map_err(map_receipt_error)?;
        match write {
            LoopsWebhookReceiptWriteOutcome::Inserted => {
                tx.commit()
                    .await
                    .map_err(|_| ApplyLoopsPreferenceEventError::Retryable)?;
                if applied {
                    Ok(ApplyLoopsPreferenceEventOutcome::CommittedApplication(
                        disposition,
                    ))
                } else {
                    Ok(ApplyLoopsPreferenceEventOutcome::Ignored(disposition))
                }
            }
            // The transaction is dropped without commit. This is important when a
            // concurrent delivery used the ID for a different mailbox or body.
            LoopsWebhookReceiptWriteOutcome::ExistingSameDigest => {
                Ok(ApplyLoopsPreferenceEventOutcome::Duplicate)
            }
            LoopsWebhookReceiptWriteOutcome::ExistingDifferentDigest => {
                Ok(ApplyLoopsPreferenceEventOutcome::Conflict)
            }
        }
    }

    fn receipt(
        &self,
        event: &VerifiedPreferenceEvent,
        disposition: Disposition,
    ) -> LoopsWebhookReceiptInput {
        LoopsWebhookReceiptInput {
            delivery_id: event.delivery_id.clone(),
            raw_body_sha256: event.raw_body_sha256,
            event_name: event.event_name.clone(),
            event_time: event.event_time,
            provider_contact_id: Some(event.contact_id.clone()),
            email: Some(event.email_text.clone()),
            mailing_list_id: event.mailing_list_id.clone(),
            disposition,
            processed_at: event.processed_at,
            expires_at: event.processed_at + WEBHOOK_RECEIPT_RETENTION,
        }
    }

    async fn apply_non_grant_event(
        &self,
        event: VerifiedPreferenceEvent,
        kind: NewsletterWebhookEventKind,
    ) -> Result<ApplyLoopsPreferenceEventOutcome, ApplyLoopsPreferenceEventError> {
        let classification = self.classify(kind, event.mailing_list_id.as_deref());
        let mut tx = self
            .unit_of_work
            .begin()
            .await
            .map_err(|_| ApplyLoopsPreferenceEventError::Retryable)?;
        if let Some(existing) = self
            .lookup_delivery(&mut tx, &event.delivery_id, &event.raw_body_sha256)
            .await?
        {
            return Ok(existing);
        }
        self.lock_recipient(&mut tx, &event.email).await?;
        // A competing request may have inserted the same delivery ID before this
        // request acquired the mailbox lock.
        if let Some(existing) = self
            .lookup_delivery(&mut tx, &event.delivery_id, &event.raw_body_sha256)
            .await?
        {
            return Ok(existing);
        }

        let fence = self.find_fence(&mut tx, &event.email).await?;
        let disposition = match classification {
            EventClassification::Ignore(disposition) => disposition,
            EventClassification::Withdraw(disposition) => {
                if fence
                    .as_ref()
                    .is_some_and(|fence| fence.provider_contact_id != event.contact_id)
                {
                    Disposition::IgnoredContactMismatch
                } else if fence
                    .as_ref()
                    .is_some_and(|fence| event.event_time < fence.latest_event_at)
                {
                    Disposition::IgnoredStale
                } else {
                    let user = self
                        .intents
                        .in_transaction(&mut tx)
                        .find_user_by_email(&event.email)
                        .await
                        .map_err(map_intent_error)?;
                    if let Some(user) = user.as_ref() {
                        if !valid_user_consent_state(user) {
                            return Err(ApplyLoopsPreferenceEventError::Retryable);
                        }
                        if user.marketing_email_consent
                            && user
                                .marketing_email_consent_changed_at
                                .is_some_and(|changed_at| event.event_time < changed_at)
                        {
                            Disposition::IgnoredStale
                        } else {
                            self.apply_withdrawal(&mut tx, event.email.clone(), event.event_time)
                                .await?;
                            self.advance_fence(
                                &mut tx,
                                &event.email,
                                &event.contact_id,
                                event.event_time,
                                false,
                            )
                            .await?;
                            disposition
                        }
                    } else {
                        self.apply_withdrawal(&mut tx, event.email.clone(), event.event_time)
                            .await?;
                        self.advance_fence(
                            &mut tx,
                            &event.email,
                            &event.contact_id,
                            event.event_time,
                            false,
                        )
                        .await?;
                        disposition
                    }
                }
            }
        };
        let applied = matches!(
            disposition,
            Disposition::AppliedWithdrawal
                | Disposition::AppliedContactRemoval
                | Disposition::AppliedComplaintBlock
        );
        let receipt = self.receipt(&event, disposition);
        self.insert_and_commit(tx, receipt, applied).await
    }

    fn classify(
        &self,
        kind: NewsletterWebhookEventKind,
        mailing_list_id: Option<&str>,
    ) -> EventClassification {
        use NewsletterWebhookEventKind as Kind;
        match kind {
            Kind::ContactUnsubscribed | Kind::EmailUnsubscribed => {
                EventClassification::Withdraw(Disposition::AppliedWithdrawal)
            }
            Kind::ContactDeleted => {
                EventClassification::Withdraw(Disposition::AppliedContactRemoval)
            }
            Kind::MailingListUnsubscribed
                if mailing_list_id == Some(self.target_list_id.as_str()) =>
            {
                EventClassification::Withdraw(Disposition::AppliedWithdrawal)
            }
            Kind::MailingListUnsubscribed => {
                EventClassification::Ignore(Disposition::IgnoredUnrelatedList)
            }
            Kind::MailingListSubscribed => EventClassification::Ignore(Disposition::IgnoredApiEcho),
            Kind::EmailHardBounced => EventClassification::Ignore(Disposition::IgnoredHardBounce),
            Kind::EmailSpamReported => {
                EventClassification::Withdraw(Disposition::AppliedComplaintBlock)
            }
            Kind::EmailResubscribed => {
                EventClassification::Ignore(Disposition::IgnoredProviderState)
            }
        }
    }

    async fn apply_withdrawal(
        &self,
        tx: &mut U::Tx,
        email: Email,
        event_time: OffsetDateTime,
    ) -> Result<(), ApplyLoopsPreferenceEventError> {
        MarketingConsentCoordinator::new(tx, &self.intents)
            .provider_withdrawal(email, event_time)
            .await
            .map_err(map_consent_error)
    }

    async fn advance_fence(
        &self,
        tx: &mut U::Tx,
        email: &Email,
        contact_id: &str,
        event_time: OffsetDateTime,
        subscribed: bool,
    ) -> Result<(), ApplyLoopsPreferenceEventError> {
        self.receipts
            .in_transaction(tx)
            .advance_preference_fence(email, contact_id, event_time, subscribed)
            .await
            .map_err(map_receipt_error)
    }

    async fn apply_resubscription(
        &self,
        event: VerifiedPreferenceEvent,
    ) -> Result<ApplyLoopsPreferenceEventOutcome, ApplyLoopsPreferenceEventError> {
        // Capture the exact mailbox decision under C02's recipient lock, then
        // commit/release before the bounded provider read.
        let mut tx = self
            .unit_of_work
            .begin()
            .await
            .map_err(|_| ApplyLoopsPreferenceEventError::Retryable)?;
        if let Some(existing) = self
            .lookup_delivery(&mut tx, &event.delivery_id, &event.raw_body_sha256)
            .await?
        {
            return Ok(existing);
        }
        self.lock_recipient(&mut tx, &event.email).await?;
        if let Some(existing) = self
            .lookup_delivery(&mut tx, &event.delivery_id, &event.raw_body_sha256)
            .await?
        {
            return Ok(existing);
        }
        let fence = self.find_fence(&mut tx, &event.email).await?;
        if fence
            .as_ref()
            .is_some_and(|fence| fence.provider_contact_id != event.contact_id)
        {
            let receipt = self.receipt(&event, Disposition::IgnoredContactMismatch);
            return self.insert_and_commit(tx, receipt, false).await;
        }
        if fence
            .as_ref()
            .is_some_and(|fence| event.event_time <= fence.latest_event_at)
        {
            let receipt = self.receipt(&event, Disposition::IgnoredStale);
            return self.insert_and_commit(tx, receipt, false).await;
        }
        let Some(user) = self
            .intents
            .in_transaction(&mut tx)
            .find_user_by_email(&event.email)
            .await
            .map_err(map_intent_error)?
        else {
            let receipt = self.receipt(&event, Disposition::IgnoredNoRegisteredUser);
            return self.insert_and_commit(tx, receipt, false).await;
        };
        if !valid_user_consent_state(&user) {
            return Err(ApplyLoopsPreferenceEventError::Retryable);
        }
        if user
            .marketing_email_consent_changed_at
            .is_some_and(|changed_at| event.event_time <= changed_at)
        {
            let receipt = self.receipt(&event, Disposition::IgnoredStale);
            return self.insert_and_commit(tx, receipt, false).await;
        }
        let captured = CapturedConsentFence::from(&user);
        tx.commit()
            .await
            .map_err(|_| ApplyLoopsPreferenceEventError::Retryable)?;

        let provider_state = self
            .provider
            .current_state(&event.email)
            .await
            .map_err(|_| ApplyLoopsPreferenceEventError::Retryable)?;
        let eligible = match provider_state {
            MarketingEmailSubscriptionState::Present {
                contact_id: current_contact_id,
                globally_subscribed,
                on_target_list,
                suppressed,
            } if current_contact_id == event.contact_id => {
                globally_subscribed && on_target_list && !suppressed
            }
            _ => false,
        };
        if !eligible {
            return self
                .commit_ignored_resubscription(event, Disposition::IgnoredProviderState)
                .await;
        }

        let mut tx = self
            .unit_of_work
            .begin()
            .await
            .map_err(|_| ApplyLoopsPreferenceEventError::Retryable)?;
        if let Some(existing) = self
            .lookup_delivery(&mut tx, &event.delivery_id, &event.raw_body_sha256)
            .await?
        {
            return Ok(existing);
        }
        self.lock_recipient(&mut tx, &event.email).await?;
        if let Some(existing) = self
            .lookup_delivery(&mut tx, &event.delivery_id, &event.raw_body_sha256)
            .await?
        {
            return Ok(existing);
        }
        let latest_fence = self.find_fence(&mut tx, &event.email).await?;
        let latest_user = self
            .intents
            .in_transaction(&mut tx)
            .find_user_by_email(&event.email)
            .await
            .map_err(map_intent_error)?;
        let disposition = match latest_user {
            Some(user)
                if user.user_id == captured.user_id
                    && captured.matches(&user)
                    && valid_user_consent_state(&user)
                    && latest_fence.as_ref().is_none_or(|fence| {
                        fence.provider_contact_id == event.contact_id
                            && event.event_time > fence.latest_event_at
                    })
                    && user
                        .marketing_email_consent_changed_at
                        .is_none_or(|changed_at| event.event_time > changed_at) =>
            {
                self.intents
                    .in_transaction(&mut tx)
                    .apply_provider_resubscription(&user, event.event_time)
                    .await
                    .map_err(map_intent_error)?;
                self.advance_fence(
                    &mut tx,
                    &event.email,
                    &event.contact_id,
                    event.event_time,
                    true,
                )
                .await?;
                Disposition::AppliedResubscription
            }
            _ => Disposition::IgnoredStale,
        };
        let applied = disposition == Disposition::AppliedResubscription;
        let receipt = self.receipt(&event, disposition);
        self.insert_and_commit(tx, receipt, applied).await
    }

    async fn commit_ignored_resubscription(
        &self,
        event: VerifiedPreferenceEvent,
        disposition: Disposition,
    ) -> Result<ApplyLoopsPreferenceEventOutcome, ApplyLoopsPreferenceEventError> {
        let mut tx = self
            .unit_of_work
            .begin()
            .await
            .map_err(|_| ApplyLoopsPreferenceEventError::Retryable)?;
        if let Some(existing) = self
            .lookup_delivery(&mut tx, &event.delivery_id, &event.raw_body_sha256)
            .await?
        {
            return Ok(existing);
        }
        self.lock_recipient(&mut tx, &event.email).await?;
        if let Some(existing) = self
            .lookup_delivery(&mut tx, &event.delivery_id, &event.raw_body_sha256)
            .await?
        {
            return Ok(existing);
        }
        let receipt = self.receipt(&event, disposition);
        self.insert_and_commit(tx, receipt, false).await
    }
}

struct VerifiedPreferenceEvent {
    delivery_id: String,
    raw_body_sha256: [u8; 32],
    event_name: String,
    event_time: OffsetDateTime,
    email_text: String,
    email: Email,
    contact_id: String,
    mailing_list_id: Option<String>,
    processed_at: OffsetDateTime,
}

#[derive(Clone, Copy)]
enum EventClassification {
    Withdraw(Disposition),
    Ignore(Disposition),
}

#[derive(Clone, Copy)]
struct CapturedConsentFence {
    user_id: UserId,
    revision: i64,
    changed_at: Option<OffsetDateTime>,
    consent: bool,
}

impl From<&ConsentUser> for CapturedConsentFence {
    fn from(user: &ConsentUser) -> Self {
        Self {
            user_id: user.user_id,
            revision: user.marketing_email_consent_revision,
            changed_at: user.marketing_email_consent_changed_at,
            consent: user.marketing_email_consent,
        }
    }
}

impl CapturedConsentFence {
    fn matches(self, user: &ConsentUser) -> bool {
        self.revision == user.marketing_email_consent_revision
            && self.changed_at == user.marketing_email_consent_changed_at
            && self.consent == user.marketing_email_consent
    }
}

fn valid_user_consent_state(user: &ConsentUser) -> bool {
    user.marketing_email_consent_revision >= 0
        && (user.marketing_email_consent_revision == 0
            || user.marketing_email_consent_changed_at.is_some())
}

fn exact_email(value: &str) -> Option<Email> {
    let email = Email::try_from(value).ok()?;
    (<Email as AsRef<str>>::as_ref(&email) == value).then_some(email)
}

fn event_time(unix_seconds: i64) -> Result<OffsetDateTime, ApplyLoopsPreferenceEventError> {
    OffsetDateTime::from_unix_timestamp(unix_seconds)
        .map_err(|_| ApplyLoopsPreferenceEventError::InvalidEventTime)
}

fn match_digest(
    existing: LoopsWebhookReceiptLookup,
    expected: &[u8; 32],
) -> ApplyLoopsPreferenceEventOutcome {
    if &existing.raw_body_sha256 == expected {
        ApplyLoopsPreferenceEventOutcome::Duplicate
    } else {
        ApplyLoopsPreferenceEventOutcome::Conflict
    }
}

fn map_intent_error(_: MarketingConsentIntentError) -> ApplyLoopsPreferenceEventError {
    ApplyLoopsPreferenceEventError::Retryable
}

fn map_receipt_error(_: LoopsWebhookReceiptError) -> ApplyLoopsPreferenceEventError {
    ApplyLoopsPreferenceEventError::Retryable
}

fn map_consent_error(_: CoordinateMarketingConsentError) -> ApplyLoopsPreferenceEventError {
    ApplyLoopsPreferenceEventError::Retryable
}

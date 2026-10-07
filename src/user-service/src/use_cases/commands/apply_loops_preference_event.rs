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
use crate::use_cases::commands::marketing_consent_evidence::{
    ConsentEvidenceAction, ConsentEvidenceSource, MarketingConsentEvidence,
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
        evidence: Option<MarketingConsentEvidence>,
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
                if let Some(evidence) = evidence {
                    evidence.emit_after_commit(None);
                }
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
        let mut evidence = None;
        let disposition = match classification {
            EventClassification::Ignore(disposition) => disposition,
            EventClassification::Withdraw(disposition) => {
                if fence
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
                                .is_some_and(|changed_at| {
                                    event.event_time.unix_timestamp() < changed_at.unix_timestamp()
                                })
                        {
                            Disposition::IgnoredStale
                        } else {
                            self.apply_withdrawal(&mut tx, event.email.clone(), event.event_time)
                                .await?;
                            evidence = provider_withdrawal_evidence(
                                &event,
                                kind,
                                Some(user),
                                disposition,
                            )?;
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
                        evidence = provider_withdrawal_evidence(&event, kind, None, disposition)?;
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
        self.insert_and_commit(tx, receipt, applied, evidence).await
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
            .is_some_and(|fence| event.event_time <= fence.latest_event_at)
        {
            let receipt = self.receipt(&event, Disposition::IgnoredStale);
            return self.insert_and_commit(tx, receipt, false, None).await;
        }
        // A replacement contact ID is eligible only when the later provider read
        // confirms this exact mailbox now resolves to the event's contact. Keep
        // the captured fence so the final transaction can reject an intervening
        // provider decision/contact rotation.
        let captured_provider_fence = fence;
        let Some(user) = self
            .intents
            .in_transaction(&mut tx)
            .find_user_by_email(&event.email)
            .await
            .map_err(map_intent_error)?
        else {
            let receipt = self.receipt(&event, Disposition::IgnoredNoRegisteredUser);
            return self.insert_and_commit(tx, receipt, false, None).await;
        };
        if !valid_user_consent_state(&user) {
            return Err(ApplyLoopsPreferenceEventError::Retryable);
        }
        if user
            .marketing_email_consent_changed_at
            .is_some_and(|changed_at| event.event_time <= changed_at)
        {
            let receipt = self.receipt(&event, Disposition::IgnoredStale);
            return self.insert_and_commit(tx, receipt, false, None).await;
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
        let mut evidence = None;
        let disposition = match latest_user {
            Some(user)
                if user.user_id == captured.user_id
                    && captured.matches(&user)
                    && valid_user_consent_state(&user)
                    && latest_fence == captured_provider_fence
                    && latest_fence
                        .as_ref()
                        .is_none_or(|fence| event.event_time > fence.latest_event_at)
                    && user
                        .marketing_email_consent_changed_at
                        .is_none_or(|changed_at| event.event_time > changed_at) =>
            {
                self.intents
                    .in_transaction(&mut tx)
                    .apply_provider_resubscription(&user, event.event_time)
                    .await
                    .map_err(map_intent_error)?;
                let revision = if user.marketing_email_consent {
                    user.marketing_email_consent_revision
                } else {
                    user.marketing_email_consent_revision
                        .checked_add(1)
                        .ok_or(ApplyLoopsPreferenceEventError::Retryable)?
                };
                evidence = Some(MarketingConsentEvidence::user_transition(
                    ConsentEvidenceSource::LoopsUserPreference,
                    ConsentEvidenceAction::Resubscribe,
                    user.user_id,
                    &event.email,
                    user.marketing_email_consent,
                    true,
                    event.delivery_id.clone(),
                    revision,
                    event.event_time,
                    "und",
                ));
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
        self.insert_and_commit(tx, receipt, applied, evidence).await
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
        self.insert_and_commit(tx, receipt, false, None).await
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

fn provider_withdrawal_evidence(
    event: &VerifiedPreferenceEvent,
    kind: NewsletterWebhookEventKind,
    user: Option<&ConsentUser>,
    disposition: Disposition,
) -> Result<Option<MarketingConsentEvidence>, ApplyLoopsPreferenceEventError> {
    if !matches!(
        disposition,
        Disposition::AppliedWithdrawal | Disposition::AppliedContactRemoval
    ) {
        return Ok(None);
    }
    let (source, action) = if kind == NewsletterWebhookEventKind::ContactDeleted {
        (
            ConsentEvidenceSource::ProviderContactRemoved,
            ConsentEvidenceAction::Remove,
        )
    } else {
        (
            ConsentEvidenceSource::LoopsUserPreference,
            ConsentEvidenceAction::Revoke,
        )
    };
    let evidence = match user {
        Some(user) => {
            let revision = if user.marketing_email_consent {
                user.marketing_email_consent_revision
                    .checked_add(1)
                    .ok_or(ApplyLoopsPreferenceEventError::Retryable)?
            } else {
                user.marketing_email_consent_revision
            };
            MarketingConsentEvidence::user_transition(
                source,
                action,
                user.user_id,
                &event.email,
                user.marketing_email_consent,
                false,
                event.delivery_id.clone(),
                revision,
                event.event_time,
                "und",
            )
        }
        None => MarketingConsentEvidence::email_only(
            source,
            action,
            &event.email,
            event.delivery_id.clone(),
            event.event_time,
            "und",
        ),
    };
    Ok(Some(evidence))
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

#[cfg(test)]
mod evidence_tests {
    use super::*;

    fn event() -> VerifiedPreferenceEvent {
        let email = Email::try_from("private.recipient@example.test").unwrap();
        VerifiedPreferenceEvent {
            delivery_id: "delivery-1".to_owned(),
            raw_body_sha256: [0; 32],
            event_name: "email.spam_reported".to_owned(),
            event_time: OffsetDateTime::UNIX_EPOCH,
            email_text: <Email as AsRef<str>>::as_ref(&email).to_owned(),
            email,
            contact_id: "provider-contact-1".to_owned(),
            mailing_list_id: None,
            processed_at: OffsetDateTime::UNIX_EPOCH,
        }
    }

    #[test]
    fn provider_blocks_and_delivery_failures_do_not_create_withdrawal_evidence() {
        assert!(
            provider_withdrawal_evidence(
                &event(),
                NewsletterWebhookEventKind::EmailSpamReported,
                None,
                Disposition::AppliedComplaintBlock,
            )
            .unwrap()
            .is_none()
        );
        assert!(
            provider_withdrawal_evidence(
                &event(),
                NewsletterWebhookEventKind::EmailHardBounced,
                None,
                Disposition::IgnoredHardBounce,
            )
            .unwrap()
            .is_none()
        );
        assert!(
            provider_withdrawal_evidence(
                &event(),
                NewsletterWebhookEventKind::EmailUnsubscribed,
                None,
                Disposition::IgnoredStale,
            )
            .unwrap()
            .is_none()
        );
    }
}

#[cfg(test)]
mod handler_evidence_tests {
    use super::*;
    use crate::ports::{
        ConsentIntent, GrantRaceRepairOutcome, LoopsWebhookReceipts, MarketingConsentIntentError,
        MarketingConsentIntents, MarketingEmailConsentError, MarketingEmailConsentOutcome,
        NewsletterProfile, NewsletterWebhookDeliveryId, NewsletterWebhookEmailAddress,
        NewsletterWebhookEventName, NewsletterWebhookProviderContactId,
        NewsletterWebhookRawBodySha256, UserStorageVersion, VerifiedNewsletterWebhookEvent,
    };
    use application::transaction::TransactionError;
    use std::io::{self, Write};
    use std::sync::{Arc, Mutex};
    use tracing_subscriber::fmt::MakeWriter;
    use user_core::marketing_consent_sync_intent_id::MarketingConsentSyncIntentId;

    const ADDRESS: &str = "private.recipient@example.test";
    const EVENT_AT: i64 = 1_700_000_000;
    type Shared = Arc<Mutex<State>>;

    #[derive(Default)]
    struct State {
        user: Option<ConsentUser>,
        receipts: Vec<(String, [u8; 32])>,
        commits: usize,
        fail_on_commit: Option<usize>,
        provider_reads: usize,
    }

    fn lock(state: &Shared) -> std::sync::MutexGuard<'_, State> {
        state.lock().unwrap()
    }

    #[derive(Clone)]
    struct Fake(Shared);
    struct Tx {
        state: Shared,
        pending_receipt: Option<(String, [u8; 32])>,
    }

    #[async_trait::async_trait]
    impl Transaction for Tx {
        async fn commit(self) -> Result<(), TransactionError> {
            let mut state = lock(&self.state);
            state.commits += 1;
            if state.fail_on_commit == Some(state.commits) {
                return Err(TransactionError::CommitFailed);
            }
            if let Some(receipt) = self.pending_receipt {
                state.receipts.push(receipt);
            }
            Ok(())
        }
    }

    #[async_trait::async_trait]
    impl UnitOfWork for Fake {
        type Tx = Tx;
        async fn begin(&self) -> Result<Tx, TransactionError> {
            Ok(Tx {
                state: self.0.clone(),
                pending_receipt: None,
            })
        }
    }

    impl LoopsWebhookReceiptsFactory<Tx> for Fake {
        fn in_transaction<'tx>(&'tx self, tx: &'tx mut Tx) -> impl LoopsWebhookReceipts + 'tx {
            ReceiptPort(tx)
        }
    }
    struct ReceiptPort<'a>(&'a mut Tx);

    #[async_trait::async_trait]
    impl LoopsWebhookReceipts for ReceiptPort<'_> {
        async fn find_by_delivery_id(
            &mut self,
            id: &str,
        ) -> Result<Option<LoopsWebhookReceiptLookup>, LoopsWebhookReceiptError> {
            Ok(lock(&self.0.state)
                .receipts
                .iter()
                .find(|(stored, _)| stored == id)
                .map(|(_, digest)| LoopsWebhookReceiptLookup {
                    raw_body_sha256: *digest,
                }))
        }
        async fn find_preference_fence(
            &mut self,
            _: &Email,
        ) -> Result<Option<LoopsPreferenceFence>, LoopsWebhookReceiptError> {
            Ok(None)
        }
        async fn advance_preference_fence(
            &mut self,
            _: &Email,
            _: &str,
            _: OffsetDateTime,
            _: bool,
        ) -> Result<(), LoopsWebhookReceiptError> {
            Ok(())
        }
        async fn insert_receipt(
            &mut self,
            receipt: LoopsWebhookReceiptInput,
        ) -> Result<LoopsWebhookReceiptWriteOutcome, LoopsWebhookReceiptError> {
            self.0.pending_receipt = Some((receipt.delivery_id, receipt.raw_body_sha256));
            Ok(LoopsWebhookReceiptWriteOutcome::Inserted)
        }
    }

    impl MarketingConsentIntentsFactory<Tx> for Fake {
        fn in_transaction<'tx>(&'tx self, tx: &'tx mut Tx) -> impl MarketingConsentIntents + 'tx {
            IntentPort(tx.state.clone())
        }
    }
    struct IntentPort(Shared);

    #[async_trait::async_trait]
    impl MarketingConsentIntents for IntentPort {
        async fn lock_recipient(&mut self, _: &Email) -> Result<(), MarketingConsentIntentError> {
            Ok(())
        }
        async fn lock_source_key(&mut self, _: &str) -> Result<(), MarketingConsentIntentError> {
            unreachable!()
        }
        async fn find_by_source_key(
            &mut self,
            _: &str,
        ) -> Result<Option<ConsentIntent>, MarketingConsentIntentError> {
            unreachable!()
        }
        async fn find_user_by_id(
            &mut self,
            _: UserId,
        ) -> Result<Option<ConsentUser>, MarketingConsentIntentError> {
            unreachable!()
        }
        async fn find_user_by_email(
            &mut self,
            email: &Email,
        ) -> Result<Option<ConsentUser>, MarketingConsentIntentError> {
            Ok(lock(&self.0)
                .user
                .as_ref()
                .filter(|user| &user.email == email)
                .cloned())
        }
        async fn record_user_transition(
            &mut self,
            _: &ConsentUser,
            _: bool,
            _: crate::ports::ConsentIntentSource,
            _: &str,
            _: Option<NewsletterProfile>,
            _: OffsetDateTime,
        ) -> Result<ConsentIntent, MarketingConsentIntentError> {
            unreachable!()
        }
        async fn record_email_only_intent(
            &mut self,
            _: &Email,
            _: bool,
            _: crate::ports::ConsentIntentSource,
            _: &str,
            _: Option<NewsletterProfile>,
            _: OffsetDateTime,
        ) -> Result<ConsentIntent, MarketingConsentIntentError> {
            unreachable!()
        }
        async fn apply_provider_withdrawal(
            &mut self,
            _: &ConsentUser,
            _: OffsetDateTime,
        ) -> Result<(), MarketingConsentIntentError> {
            Ok(())
        }
        async fn apply_provider_resubscription(
            &mut self,
            _: &ConsentUser,
            _: OffsetDateTime,
        ) -> Result<(), MarketingConsentIntentError> {
            Ok(())
        }
        async fn cancel_provider_backsync(
            &mut self,
            _: &Email,
        ) -> Result<(), MarketingConsentIntentError> {
            Ok(())
        }
        async fn invalidate_newsletter_confirmation_challenges(
            &mut self,
            _: &Email,
            _: OffsetDateTime,
        ) -> Result<(), MarketingConsentIntentError> {
            Ok(())
        }
        async fn repair_raced_grant_if_needed(
            &mut self,
            _: MarketingConsentSyncIntentId,
            _: &str,
            _: OffsetDateTime,
        ) -> Result<GrantRaceRepairOutcome, MarketingConsentIntentError> {
            unreachable!()
        }
        async fn record_user_deletion(
            &mut self,
            _: &ConsentUser,
            _: &str,
            _: OffsetDateTime,
        ) -> Result<ConsentIntent, MarketingConsentIntentError> {
            unreachable!()
        }
    }

    #[async_trait::async_trait]
    impl MarketingEmailConsentWriter for Fake {
        async fn current_state(
            &self,
            _: &Email,
        ) -> Result<MarketingEmailSubscriptionState, MarketingEmailConsentError> {
            lock(&self.0).provider_reads += 1;
            Ok(MarketingEmailSubscriptionState::Present {
                contact_id: "provider-contact-1".to_owned(),
                globally_subscribed: true,
                on_target_list: true,
                suppressed: false,
            })
        }
        async fn grant(
            &self,
            _: &ConsentIntent,
        ) -> Result<MarketingEmailConsentOutcome, MarketingEmailConsentError> {
            unreachable!()
        }
        async fn revoke(
            &self,
            _: &ConsentIntent,
        ) -> Result<MarketingEmailConsentOutcome, MarketingEmailConsentError> {
            unreachable!()
        }
    }

    // The writer runs synchronously on the test thread, so it can assert the
    // transaction has completed before the first byte of evidence is emitted.
    #[derive(Clone)]
    struct CommittedWriter {
        state: Shared,
        output: Arc<Mutex<Vec<u8>>>,
        required_commits: usize,
    }
    impl<'a> MakeWriter<'a> for CommittedWriter {
        type Writer = Self;
        fn make_writer(&'a self) -> Self::Writer {
            self.clone()
        }
    }
    impl Write for CommittedWriter {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            let state = lock(&self.state);
            assert!(state.commits >= self.required_commits);
            assert_eq!(
                state.receipts.len(),
                1,
                "evidence must follow receipt commit"
            );
            drop(state);
            self.output.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    fn command(name: &str, kind: NewsletterWebhookEventKind) -> ApplyLoopsPreferenceEventCommand {
        ApplyLoopsPreferenceEventCommand {
            verification: NewsletterWebhookVerification {
                raw_body_sha256: NewsletterWebhookRawBodySha256::new([7; 32]),
                outcome: NewsletterWebhookVerificationOutcome::Verified(
                    VerifiedNewsletterWebhookEvent {
                        delivery_id: NewsletterWebhookDeliveryId::new("delivery-1").unwrap(),
                        provider_event_name: NewsletterWebhookEventName::new(name).unwrap(),
                        kind,
                        event_time_unix_seconds: EVENT_AT,
                        provider_contact_id: NewsletterWebhookProviderContactId::new(
                            "provider-contact-1",
                        )
                        .unwrap(),
                        email: NewsletterWebhookEmailAddress::new(ADDRESS).unwrap(),
                        mailing_list_id: None,
                    },
                ),
            },
        }
    }

    fn execute_captured(
        state: &Shared,
        command: ApplyLoopsPreferenceEventCommand,
        required_commits: usize,
    ) -> (
        Result<ApplyLoopsPreferenceEventOutcome, ApplyLoopsPreferenceEventError>,
        String,
    ) {
        let output = Arc::new(Mutex::new(Vec::new()));
        let subscriber = tracing_subscriber::fmt()
            .json()
            .with_ansi(false)
            .with_writer(CommittedWriter {
                state: state.clone(),
                output: output.clone(),
                required_commits,
            })
            .finish();
        let fake = Fake(state.clone());
        let handler = ApplyLoopsPreferenceEventHandler::new(
            fake.clone(),
            fake.clone(),
            fake.clone(),
            fake,
            NewsletterWebhookMailingListId::new("marketing-list").unwrap(),
        );
        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();
        let outcome = tracing::subscriber::with_default(subscriber, || {
            runtime.block_on(handler.execute(command))
        });
        let line = String::from_utf8(output.lock().unwrap().clone()).unwrap();
        (outcome, line)
    }

    fn user(consent: bool, revision: i64) -> ConsentUser {
        ConsentUser {
            user_id: UserId::new(),
            email: Email::try_from(ADDRESS).unwrap(),
            version: UserStorageVersion::INITIAL,
            marketing_email_consent: consent,
            marketing_email_consent_revision: revision,
            marketing_email_consent_changed_at: Some(
                OffsetDateTime::from_unix_timestamp(EVENT_AT - 10).unwrap(),
            ),
        }
    }

    fn assert_evidence(
        line: &str,
        source: &str,
        action: &str,
        user: &ConsentUser,
        previous: bool,
        current: bool,
        revision: i64,
    ) {
        let record: serde_json::Value = serde_json::from_str(line).unwrap();
        let fields = &record["fields"];
        assert_eq!(fields["event"], "marketing_consent.evidence.v1");
        assert_eq!(fields["consent_purpose"], "EMAIL_MARKETING");
        assert_eq!(fields["consent_source"], source);
        assert_eq!(fields["consent_action"], action);
        assert_eq!(fields["subject_kind"], "USER");
        assert_eq!(fields["user_id"], user.user_id.to_string());
        assert_eq!(
            fields["recipient_fingerprint"],
            crate::ports::marketing_consent_recipient_key(&user.email)
        );
        assert_eq!(fields["consent_decision_id"], "delivery-1");
        assert_eq!(fields["previous_consent"], previous);
        assert_eq!(fields["current_consent"], current);
        assert_eq!(fields["consent_revision"], revision);
        assert_eq!(fields["consent_effective_at_utc"], "2023-11-14T22:13:20Z");
        let recorded = fields["consent_recorded_at_utc"].as_str().unwrap();
        assert!(
            OffsetDateTime::parse(recorded, &time::format_description::well_known::Rfc3339).is_ok()
        );
        assert!(recorded.ends_with('Z'));
        assert_eq!(fields["consent_wording_reference"], "not-recorded");
        assert_eq!(fields["consent_wording_locale"], "und");

        assert_eq!(fields["request_id"], "");
        assert_eq!(fields["correlation_id"], "");
        for secret in [
            ADDRESS,
            "provider-contact-1",
            "raw_token",
            "signature",
            "provider_payload",
        ] {
            assert!(!line.contains(secret));
        }
    }

    #[test]
    fn committed_unsubscribe_and_contact_deletion_log_distinct_evidence_once() {
        for (name, kind, disposition, source, action) in [
            (
                "contact.unsubscribed",
                NewsletterWebhookEventKind::ContactUnsubscribed,
                Disposition::AppliedWithdrawal,
                "LOOPS_USER_PREFERENCE",
                "REVOKE",
            ),
            (
                "contact.deleted",
                NewsletterWebhookEventKind::ContactDeleted,
                Disposition::AppliedContactRemoval,
                "PROVIDER_CONTACT_REMOVED",
                "REMOVE",
            ),
        ] {
            let user = user(true, 4);
            let state = Arc::new(Mutex::new(State {
                user: Some(user.clone()),
                ..State::default()
            }));
            let (outcome, line) = execute_captured(&state, command(name, kind), 1);
            assert_eq!(
                outcome,
                Ok(ApplyLoopsPreferenceEventOutcome::CommittedApplication(
                    disposition
                ))
            );
            assert_evidence(&line, source, action, &user, true, false, 5);
            assert_eq!(lock(&state).receipts.len(), 1);
            let (duplicate, replay_log) = execute_captured(&state, command(name, kind), 2);
            assert_eq!(duplicate, Ok(ApplyLoopsPreferenceEventOutcome::Duplicate));
            assert!(replay_log.is_empty());
            assert_eq!(lock(&state).commits, 1);
        }
    }

    #[test]
    fn anonymous_unsubscribe_logs_email_only_fields() {
        let state = Arc::new(Mutex::new(State::default()));
        let (outcome, line) = execute_captured(
            &state,
            command(
                "contact.unsubscribed",
                NewsletterWebhookEventKind::ContactUnsubscribed,
            ),
            1,
        );
        assert_eq!(
            outcome,
            Ok(ApplyLoopsPreferenceEventOutcome::CommittedApplication(
                Disposition::AppliedWithdrawal
            ))
        );
        let record: serde_json::Value = serde_json::from_str(&line).unwrap();
        let fields = &record["fields"];
        assert_eq!(fields["event"], "marketing_consent.evidence.v1");
        assert_eq!(fields["consent_source"], "LOOPS_USER_PREFERENCE");
        assert_eq!(fields["consent_action"], "REVOKE");
        assert_eq!(fields["subject_kind"], "EMAIL_ONLY");
        assert_eq!(fields["consent_wording_reference"], "not-recorded");
        assert_eq!(fields["consent_wording_locale"], "und");
        assert_eq!(fields["consent_decision_id"], "delivery-1");
        assert_eq!(
            fields["recipient_fingerprint"],
            crate::ports::marketing_consent_recipient_key(&Email::try_from(ADDRESS).unwrap())
        );
        for absent in [
            "user_id",
            "previous_consent",
            "current_consent",
            "consent_revision",
        ] {
            assert!(fields.get(absent).is_none());
        }
        assert!(!line.contains(ADDRESS));
    }

    #[test]
    fn committed_resubscription_logs_only_after_the_final_commit() {
        let user = user(false, 4);
        let state = Arc::new(Mutex::new(State {
            user: Some(user.clone()),
            ..State::default()
        }));
        let (outcome, line) = execute_captured(
            &state,
            command(
                "email.resubscribed",
                NewsletterWebhookEventKind::EmailResubscribed,
            ),
            2,
        );
        assert_eq!(
            outcome,
            Ok(ApplyLoopsPreferenceEventOutcome::CommittedApplication(
                Disposition::AppliedResubscription
            ))
        );
        assert_evidence(
            &line,
            "LOOPS_USER_PREFERENCE",
            "RESUBSCRIBE",
            &user,
            false,
            true,
            5,
        );
        assert_eq!(lock(&state).provider_reads, 1);
        assert_eq!(lock(&state).receipts.len(), 1);
        let (duplicate, replay_log) = execute_captured(
            &state,
            command(
                "email.resubscribed",
                NewsletterWebhookEventKind::EmailResubscribed,
            ),
            3,
        );
        assert_eq!(duplicate, Ok(ApplyLoopsPreferenceEventOutcome::Duplicate));
        assert!(replay_log.is_empty());
        assert_eq!(lock(&state).provider_reads, 1);
    }

    #[test]
    fn failed_application_commit_emits_no_evidence_or_receipt() {
        for (kind, name, failed_commit) in [
            (
                NewsletterWebhookEventKind::ContactUnsubscribed,
                "contact.unsubscribed",
                1,
            ),
            (
                NewsletterWebhookEventKind::ContactDeleted,
                "contact.deleted",
                1,
            ),
            (
                NewsletterWebhookEventKind::EmailResubscribed,
                "email.resubscribed",
                2,
            ),
        ] {
            let state = Arc::new(Mutex::new(State {
                user: Some(user(
                    kind != NewsletterWebhookEventKind::EmailResubscribed,
                    4,
                )),
                fail_on_commit: Some(failed_commit),
                ..State::default()
            }));
            let (outcome, line) = execute_captured(&state, command(name, kind), failed_commit + 1);
            assert_eq!(outcome, Err(ApplyLoopsPreferenceEventError::Retryable));
            assert!(line.is_empty());
            assert!(lock(&state).receipts.is_empty());
            assert_eq!(lock(&state).commits, failed_commit);

            lock(&state).fail_on_commit = None;
            let (retry, retry_log) = execute_captured(
                &state,
                command(name, kind),
                failed_commit + if failed_commit == 2 { 2 } else { 1 },
            );
            assert!(matches!(
                retry,
                Ok(ApplyLoopsPreferenceEventOutcome::CommittedApplication(_))
            ));
            assert_eq!(retry_log.lines().count(), 1);
            assert_eq!(lock(&state).receipts.len(), 1);
        }
    }
}

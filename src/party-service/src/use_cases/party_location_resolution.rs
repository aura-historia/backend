//! Protected review of a PartyLocation's durable geographic resolution.
use crate::{
    ports::{party_location::*, party_location_resolution_review_reader::*},
    use_cases::party_locations::{
        PartyLocationError, PartyLocationView, actor, authorize, load, privileged,
        resolution_binding, resolution_subject, sync_resolution,
    },
};
use application::{
    operation_context::{OperationContext, Principal},
    transaction::{Transaction, UnitOfWork},
};
use geo_service::resolution::{
    MAX_RESOLUTION_SUBJECTS, ResolutionReader, ResolutionRepository, ResolutionStatus,
    ResolutionView, ResolutionWork,
};
use party_core::{party_id::PartyId, party_location_id::PartyLocationId};
use user_service::use_cases::queries::check_user_admin::CheckUserAdminUseCase;

/// Authorized inspection and review use the same Party management policy as edits.
/// Candidate identity includes the exact resolution generation and current location CAS.
pub enum PartyLocationResolutionCommand {
    Request {
        party_id: PartyId,
        expected: ExpectedPartyLocation,
    },
    Inspect {
        party_id: PartyId,
        id: PartyLocationId,
    },
    AcceptCandidate {
        party_id: PartyId,
        expected: ExpectedPartyLocation,
        work: ResolutionWork,
        candidate_index: usize,
    },
    Retry {
        party_id: PartyId,
        expected: ExpectedPartyLocation,
        work: ResolutionWork,
        force: bool,
    },
}
#[async_trait::async_trait]
pub trait ManagePartyLocationResolutionUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        command: PartyLocationResolutionCommand,
    ) -> Result<Option<ResolutionView>, PartyLocationError>;
}
/// Who may review a location's resolution.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PartyLocationReviewPolicy {
    /// The Party management policy shared with location edits.
    AdministratorOrManager,
    /// Administrator routes refuse location managers who are not administrators.
    AdministratorOnly,
}
pub struct ManagePartyLocationResolutionHandler<U, R, F, A> {
    uow: U,
    locations: R,
    access: F,
    admin: A,
    policy: PartyLocationReviewPolicy,
}
impl<U, R, F, A> ManagePartyLocationResolutionHandler<U, R, F, A> {
    pub fn new(uow: U, locations: R, access: F, admin: A) -> Self {
        Self {
            uow,
            locations,
            access,
            admin,
            policy: PartyLocationReviewPolicy::AdministratorOrManager,
        }
    }
    pub fn new_admin_only(uow: U, locations: R, access: F, admin: A) -> Self {
        Self {
            policy: PartyLocationReviewPolicy::AdministratorOnly,
            ..Self::new(uow, locations, access, admin)
        }
    }
}
#[async_trait::async_trait]
impl<
    U: UnitOfWork,
    R: PartyLocationRepositoryFactory<U::Tx>,
    F: PartyLocationAccessFactory<U::Tx>,
    A: CheckUserAdminUseCase,
> ManagePartyLocationResolutionUseCase for ManagePartyLocationResolutionHandler<U, R, F, A>
{
    async fn execute(
        &self,
        context: &OperationContext,
        command: PartyLocationResolutionCommand,
    ) -> Result<Option<ResolutionView>, PartyLocationError> {
        let action = match &command {
            PartyLocationResolutionCommand::Request { .. } => "REQUEST",
            PartyLocationResolutionCommand::Inspect { .. } => "INSPECT",
            PartyLocationResolutionCommand::AcceptCandidate { .. } => "ACCEPT_CANDIDATE",
            PartyLocationResolutionCommand::Retry { .. } => "RETRY",
        };
        let (party_id, id, write) = match &command {
            PartyLocationResolutionCommand::Inspect { party_id, id } => (*party_id, *id, false),
            PartyLocationResolutionCommand::AcceptCandidate {
                party_id, expected, ..
            }
            | PartyLocationResolutionCommand::Retry {
                party_id, expected, ..
            }
            | PartyLocationResolutionCommand::Request { party_id, expected } => {
                (*party_id, expected.id, true)
            }
        };
        let privileged = privileged(context, &self.admin, write).await?;
        if self.policy == PartyLocationReviewPolicy::AdministratorOnly && !privileged {
            return Err(PartyLocationError::Forbidden);
        }
        let mut tx = self.uow.begin().await?;
        authorize(context, privileged, &self.access, &mut tx, party_id).await?;
        match command {
            PartyLocationResolutionCommand::Request { expected, .. } => {
                let s = load(&self.locations, &mut tx, party_id, expected).await?;
                sync_resolution(&self.locations, &mut tx, &s).await?;
            }
            PartyLocationResolutionCommand::Inspect { .. } => {
                self.locations
                    .in_transaction(&mut tx)
                    .find_for_update(party_id, id)
                    .await?
                    .ok_or(PartyLocationError::NotFound)?;
            }
            PartyLocationResolutionCommand::AcceptCandidate {
                expected,
                work,
                candidate_index,
                ..
            } => {
                let s = load(&self.locations, &mut tx, party_id, expected).await?;
                self.locations
                    .resolutions(&mut tx)
                    .select(
                        resolution_binding(&s)?,
                        work,
                        candidate_index,
                        &actor(context),
                    )
                    .await?;
            }
            PartyLocationResolutionCommand::Retry {
                expected,
                work,
                force,
                ..
            } => {
                let s = load(&self.locations, &mut tx, party_id, expected).await?;
                self.locations
                    .resolutions(&mut tx)
                    .requeue(resolution_binding(&s)?, work, force)
                    .await?;
            }
        }
        let view = self
            .locations
            .resolution_reader(&mut tx)
            .inspect(resolution_subject(id)?)
            .await?;
        tx.commit().await?;
        if write {
            tracing::info!(
                event = "party_location.resolution_managed",
                action,
                actor = %actor(context),
                location_id = %id,
                request_id = %context.request_id,
                correlation_id = %context.correlation_id,
                generation = view.as_ref().map(|v| v.work.generation),
                outcome = "success"
            );
        }
        Ok(view)
    }
}

/// Resolution outcomes that no automatic step will settle: `PRECISION_FIRST_V1` never accepts
/// a Party point without review, and blocked or exhausted work needs an explicit Retry.
/// Unreviewed ambiguity counts; an ambiguous generation with a reviewed selection does not.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResolutionAttention {
    Ambiguous,
    Blocked,
    Failed,
}
impl ResolutionAttention {
    pub const ALL: [Self; 3] = [Self::Ambiguous, Self::Blocked, Self::Failed];
    pub fn status(self) -> ResolutionStatus {
        match self {
            Self::Ambiguous => ResolutionStatus::Ambiguous,
            Self::Blocked => ResolutionStatus::Blocked,
            Self::Failed => ResolutionStatus::Failed,
        }
    }
    /// A reviewed selection settles an ambiguous generation; only Retry or new input reopens it.
    pub fn needed_by(view: &ResolutionView) -> Option<Self> {
        if view.selected_candidate.is_some() {
            return None;
        }
        Self::ALL.into_iter().find(|a| a.status() == view.status)
    }
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListPartyLocationResolutionReviewsRequest {
    /// `None` lists every status that needs attention.
    pub status: Option<ResolutionAttention>,
    pub after: Option<PartyLocationId>,
    pub limit: u32,
}
#[derive(Debug, Clone)]
pub struct PartyLocationResolutionReview {
    pub location: PartyLocationView,
    pub resolution: ResolutionView,
}
#[derive(Debug, Clone)]
pub struct PartyLocationResolutionReviewsPage {
    pub items: Vec<PartyLocationResolutionReview>,
    /// Keyset continuation; present whenever later locations may match.
    pub next: Option<PartyLocationId>,
}

#[async_trait::async_trait]
pub trait ListPartyLocationResolutionReviewsUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        request: ListPartyLocationResolutionReviewsRequest,
    ) -> Result<PartyLocationResolutionReviewsPage, PartyLocationError>;
}
/// Administrator-only: the queue spans every Party, so no location-manager grant applies.
pub struct ListPartyLocationResolutionReviewsHandler<U, R, F, A> {
    uow: U,
    reader: R,
    access: F,
    admin: A,
}
impl<U, R, F, A> ListPartyLocationResolutionReviewsHandler<U, R, F, A> {
    pub fn new(uow: U, reader: R, access: F, admin: A) -> Self {
        Self {
            uow,
            reader,
            access,
            admin,
        }
    }
}
#[async_trait::async_trait]
impl<U, R, F, A> ListPartyLocationResolutionReviewsUseCase
    for ListPartyLocationResolutionReviewsHandler<U, R, F, A>
where
    U: UnitOfWork,
    R: PartyLocationResolutionReviewReaderFactory<U::Tx>,
    F: PartyLocationAccessFactory<U::Tx>,
    A: CheckUserAdminUseCase,
{
    #[tracing::instrument(skip_all, fields(principal_type = context.principal.kind(), request_id = %context.request_id, correlation_id = %context.correlation_id))]
    async fn execute(
        &self,
        context: &OperationContext,
        request: ListPartyLocationResolutionReviewsRequest,
    ) -> Result<PartyLocationResolutionReviewsPage, PartyLocationError> {
        if !(1..=MAX_RESOLUTION_SUBJECTS as u32).contains(&request.limit) {
            return Err(PartyLocationError::InvalidInput);
        }
        if !privileged(context, &self.admin, false).await? {
            return Err(PartyLocationError::Forbidden);
        }
        let mut tx = self.uow.begin().await?;
        if let Principal::User(user) | Principal::DelegatedUser { user_id: user, .. } =
            context.principal
            && !self
                .access
                .in_transaction(&mut tx)
                .can_administer_locations(user)
                .await?
        {
            return Err(PartyLocationError::Forbidden);
        }
        let statuses = request
            .status
            .map_or(ResolutionAttention::ALL.to_vec(), |status| vec![status]);
        let page = self
            .reader
            .in_transaction(&mut tx)
            .list(&statuses, request.after, request.limit)
            .await?;
        tx.commit().await?;
        Ok(page)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use application::{
        operation_context::{CorrelationId, RequestId},
        transaction::TransactionError,
    };
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };
    use user_core::user_id::UserId;
    use user_service::use_cases::queries::check_user_admin::{
        CheckUserAdminError, CheckUserAdminRequest, CheckUserAdminResult,
    };

    #[derive(Clone, Default)]
    struct Probe {
        admin: bool,
        read: Arc<AtomicBool>,
    }
    struct Tx;
    #[async_trait::async_trait]
    impl Transaction for Tx {
        async fn commit(self) -> Result<(), TransactionError> {
            Ok(())
        }
    }
    #[async_trait::async_trait]
    impl UnitOfWork for Probe {
        type Tx = Tx;
        async fn begin(&self) -> Result<Tx, TransactionError> {
            Ok(Tx)
        }
    }
    #[async_trait::async_trait]
    impl CheckUserAdminUseCase for Probe {
        async fn execute(
            &self,
            _: &OperationContext,
            _: CheckUserAdminRequest,
        ) -> Result<CheckUserAdminResult, CheckUserAdminError> {
            if self.admin {
                Ok(CheckUserAdminResult)
            } else {
                Err(CheckUserAdminError::Forbidden)
            }
        }
    }
    struct Access(bool);
    #[async_trait::async_trait]
    impl PartyLocationAccess for Access {
        async fn lock_party(&mut self, _: PartyId) -> Result<bool, PartyLocationError> {
            unreachable!("the review queue spans Parties")
        }
        async fn can_administer_locations(
            &mut self,
            _: UserId,
        ) -> Result<bool, PartyLocationError> {
            Ok(self.0)
        }
        async fn can_manage_locations(
            &mut self,
            _: PartyId,
            _: UserId,
        ) -> Result<bool, PartyLocationError> {
            unreachable!("managers never qualify for the review queue")
        }
        async fn set_management_grant(
            &mut self,
            _: PartyId,
            _: UserId,
            _: bool,
            _: &str,
        ) -> Result<(), PartyLocationError> {
            unreachable!()
        }
    }
    impl PartyLocationAccessFactory<Tx> for Probe {
        fn in_transaction<'tx>(&'tx self, _: &'tx mut Tx) -> impl PartyLocationAccess + 'tx {
            Access(self.admin)
        }
    }
    struct Reader<'a>(&'a AtomicBool);
    #[async_trait::async_trait]
    impl PartyLocationResolutionReviewReader for Reader<'_> {
        async fn list(
            &mut self,
            statuses: &[ResolutionAttention],
            _: Option<PartyLocationId>,
            _: u32,
        ) -> Result<PartyLocationResolutionReviewsPage, PartyLocationError> {
            assert!(!statuses.is_empty());
            self.0.store(true, Ordering::SeqCst);
            Ok(PartyLocationResolutionReviewsPage {
                items: vec![],
                next: None,
            })
        }
    }
    impl PartyLocationResolutionReviewReaderFactory<Tx> for Probe {
        fn in_transaction<'tx>(
            &'tx self,
            _: &'tx mut Tx,
        ) -> impl PartyLocationResolutionReviewReader + 'tx {
            Reader(&self.read)
        }
    }
    fn handler(
        probe: &Probe,
    ) -> ListPartyLocationResolutionReviewsHandler<Probe, Probe, Probe, Probe> {
        ListPartyLocationResolutionReviewsHandler::new(
            probe.clone(),
            probe.clone(),
            probe.clone(),
            probe.clone(),
        )
    }
    fn context(principal: Principal) -> OperationContext {
        OperationContext {
            principal,
            request_id: RequestId::new("resolution-review-test"),
            correlation_id: CorrelationId::new("resolution-review-test"),
        }
    }
    fn request(limit: u32) -> ListPartyLocationResolutionReviewsRequest {
        ListPartyLocationResolutionReviewsRequest {
            status: None,
            after: None,
            limit,
        }
    }

    #[tokio::test]
    async fn should_list_the_review_queue_only_for_administrators() {
        let user = Principal::User(UserId::new());
        for (principal, admin, permitted) in [
            (user.clone(), true, true),
            (Principal::Service("geo-review".to_owned()), false, true),
            (user, false, false),
        ] {
            let probe = Probe {
                admin,
                ..Probe::default()
            };
            let result = handler(&probe)
                .execute(&context(principal), request(100))
                .await;
            assert_eq!(permitted, result.is_ok(), "{result:?}");
            assert_eq!(permitted, probe.read.load(Ordering::SeqCst));
            if !permitted {
                assert!(matches!(result, Err(PartyLocationError::Forbidden)));
            }
        }
    }

    #[tokio::test]
    async fn should_reject_page_sizes_outside_one_batched_resolution_read() {
        let probe = Probe {
            admin: true,
            ..Probe::default()
        };
        for limit in [0, MAX_RESOLUTION_SUBJECTS as u32 + 1] {
            let result = handler(&probe)
                .execute(&context(Principal::System), request(limit))
                .await;
            assert!(matches!(result, Err(PartyLocationError::InvalidInput)));
        }
        assert!(!probe.read.load(Ordering::SeqCst));
    }

    #[test]
    fn should_recognise_only_unreviewed_outcomes_that_need_review() {
        let mut view = ResolutionView {
            scope: geo_service::resolution::ResolutionScope::new(
                geo_service::resolution::ScopeKind::Party,
                Some(PartyId::new().into_uuid()),
            )
            .unwrap(),
            work: ResolutionWork {
                id: geo_service::resolution::ResolutionId::new(),
                generation: 1,
            },
            input_revision: 1,
            guard: 1,
            status: ResolutionStatus::Pending,
            evidence: None,
            selected_candidate: None,
            last_failure: None,
            attempts: 0,
        };
        for status in [
            ResolutionStatus::Pending,
            ResolutionStatus::Accepted,
            ResolutionStatus::NoMatch,
        ] {
            view.status = status;
            assert_eq!(None, ResolutionAttention::needed_by(&view));
        }
        for attention in ResolutionAttention::ALL {
            view.status = attention.status();
            assert_eq!(Some(attention), ResolutionAttention::needed_by(&view));
        }
        view.status = ResolutionStatus::Ambiguous;
        view.selected_candidate = Some(0);
        assert_eq!(None, ResolutionAttention::needed_by(&view));
    }
}

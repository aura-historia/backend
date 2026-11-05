//! Protected review of a PartyLocation's durable geographic resolution.
use crate::{
    ports::party_location::*,
    use_cases::party_locations::{
        PartyLocationError, actor, authorize, load, privileged, resolution_binding,
        resolution_subject, sync_resolution,
    },
};
use application::{
    operation_context::OperationContext,
    transaction::{Transaction, UnitOfWork},
};
use geo_service::resolution::{
    ResolutionReader, ResolutionRepository, ResolutionView, ResolutionWork,
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
pub struct ManagePartyLocationResolutionHandler<U, R, F, A> {
    uow: U,
    locations: R,
    access: F,
    admin: A,
}
impl<U, R, F, A> ManagePartyLocationResolutionHandler<U, R, F, A> {
    pub fn new(uow: U, locations: R, access: F, admin: A) -> Self {
        Self {
            uow,
            locations,
            access,
            admin,
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

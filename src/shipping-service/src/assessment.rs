//! Current advertised delivery assessment. Destination snapshots stay request-local.
use application::{
    error::BoxError,
    operation_context::{CredentialCapability, OperationContext, Principal},
    transaction::{Transaction, TransactionError, UnitOfWork},
};
use geo::GeographicDescription;
use listing_source_core::ListingSourceId;
use party_core::party_location::PartyLocation;
use product_listing_core::{delivery::ListingDelivery, product_listing_id::ProductListingId};
use shipping_core::{
    assessment::{EffectiveDeliveryTerms, ShippingAssessment},
    inputs::AssessmentInput,
    profile::ShippingProfile,
    selection::{ListingDeliveryConfiguration, ProfileSelection, SourceDefaultProfile},
};
use std::sync::Arc;
use time::OffsetDateTime;
use user_core::{
    user_address::{BrowsingDestination, UserAddressId},
    user_id::UserId,
};
use user_service::address_book::{AddressBookError, AddressBookQueries, DestinationFacts};

pub const MAX_ASSESSMENT_LISTINGS: usize = 100;

/// Explicit None bypasses preference lookup, including for authenticated callers.
/// Geographic input never invokes the private address-book use case.
pub enum AssessmentDestination {
    None,
    Geographic(GeographicDescription),
    Address(UserAddressId),
    Preference,
}
pub struct AssessListingRequest {
    pub listing_id: ProductListingId,
    pub input: AssessmentInput,
}
pub struct AssessDeliveryRequest {
    pub destination: Option<AssessmentDestination>,
    pub listings: Vec<AssessListingRequest>,
}
#[derive(Debug, thiserror::Error)]
pub enum AssessmentError {
    #[error("invalid delivery assessment input")]
    InvalidInput,
    #[error("delivery assessment not permitted")]
    Forbidden,
    #[error("destination resolution failed")]
    Destination(
        #[from]
        #[source]
        AddressBookError,
    ),
    #[error("shipping assessment storage unavailable")]
    Storage(#[source] BoxError),
    #[error("invalid persisted shipping assessment dependency")]
    InvalidState(#[source] BoxError),
    #[error("shipping assessment transaction failed")]
    Transaction(
        #[from]
        #[source]
        TransactionError,
    ),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EvidenceStatus {
    Fresh,
    Stale,
    Expired,
}
impl EvidenceStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Fresh => "FRESH",
            Self::Stale => "STALE",
            Self::Expired => "EXPIRED",
        }
    }
}
/// No deadline means a continuing manually supplied assertion, not a recent recrawl.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AssertionEvidence {
    pub verified_at: OffsetDateTime,
    pub fresh_until: Option<OffsetDateTime>,
    pub expires_at: Option<OffsetDateTime>,
}
impl AssertionEvidence {
    pub fn status(self, at: OffsetDateTime) -> EvidenceStatus {
        if self.expires_at.is_some_and(|expiry| expiry <= at) {
            EvidenceStatus::Expired
        } else if self.fresh_until.is_some_and(|expiry| expiry <= at) {
            EvidenceStatus::Stale
        } else {
            EvidenceStatus::Fresh
        }
    }
}
#[derive(Clone)]
pub struct ListingDependency {
    pub id: ProductListingId,
    pub source_id: ListingSourceId,
    pub revision: u64,
    pub source_revision: u64,
    pub operator_id: party_core::party_id::PartyId,
    pub delivery: ListingDelivery,
    pub evidence: Option<AssertionEvidence>,
    pub integration_current: bool,
}
pub struct ProfileDependency {
    pub profile: ShippingProfile,
    pub management_revision: u64,
    pub integration: product_listing_core::delivery::DeliveryIntegration,
    pub evidence: AssertionEvidence,
}
pub struct DefaultDependency {
    pub selection: SourceDefaultProfile,
    pub revision: u64,
}
pub struct LocationDependency {
    pub location: PartyLocation,
    pub revision: u64,
    pub resolution_generation: Option<u64>,
}
pub struct AssessmentDependencies {
    pub evaluated_at: OffsetDateTime,
    pub listings: Vec<ListingDependency>,
    pub profiles: Vec<ProfileDependency>,
    pub defaults: Vec<DefaultDependency>,
    pub locations: Vec<LocationDependency>,
}
#[async_trait::async_trait]
pub trait AssessmentReader: Send {
    /// Read all dependencies in one coherent authoritative snapshot; bounded batch queries,
    /// with current user status checked when supplied. No projections or provider I/O.
    async fn snapshot(
        &mut self,
        ids: &[ProductListingId],
        user: Option<UserId>,
    ) -> Result<AssessmentDependencies, AssessmentError>;
}
pub trait AssessmentReaderFactory<Tx>: Send + Sync {
    fn in_transaction<'a>(&'a self, tx: &'a mut Tx) -> impl AssessmentReader + 'a;
}
/// Values only: no address-book reference, street address, point, hash or provider evidence.
pub struct DestinationSnapshot {
    pub submitted: Option<GeographicDescription>,
    pub derived: Option<GeographicDescription>,
    pub effective: Option<GeographicDescription>,
    pub attribution: Option<user_service::address_book::UserAddressAttribution>,
}
#[derive(Debug, Clone, PartialEq)]
pub struct ProfileAssessmentBasis {
    pub id: shipping_core::profile::ShippingProfileId,
    pub terms_revision: u64,
    pub management_revision: u64,
    pub lifecycle: shipping_core::profile::ShippingProfileLifecycle,
    pub evidence: AssertionEvidence,
}
pub struct ListingAssessmentView {
    pub listing_id: ProductListingId,
    pub source_id: ListingSourceId,
    pub listing_revision: u64,
    pub source_revision: u64,
    pub default_revision: Option<u64>,
    pub profile: Option<ProfileAssessmentBasis>,
    pub listing_evidence: Option<AssertionEvidence>,
    pub evaluated_at: OffsetDateTime,
    pub input: AssessmentInput,
    pub effective: EffectiveDeliveryTerms,
    /// Destination-independent evaluation for public dependency reasons.
    pub generic_assessment: ShippingAssessment,
    pub assessment: ShippingAssessment,
    pub location_revisions: Vec<LocationAssessmentBasis>,
    pub notices: Vec<&'static str>,
}
pub struct LocationAssessmentBasis {
    pub id: party_core::party_location_id::PartyLocationId,
    pub revision: u64,
    pub resolution_generation: Option<u64>,
}
pub struct DeliveryAssessmentView {
    pub destination: DestinationSnapshot,
    pub items: Vec<ListingAssessmentView>,
}
#[async_trait::async_trait]
pub trait AssessDeliveryUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        request: AssessDeliveryRequest,
    ) -> Result<DeliveryAssessmentView, AssessmentError>;
}
pub struct AssessDeliveryHandler<U, R> {
    uow: U,
    reader: R,
    destinations: Arc<dyn AddressBookQueries>,
}
impl<U, R> AssessDeliveryHandler<U, R> {
    pub fn new(uow: U, reader: R, destinations: Arc<dyn AddressBookQueries>) -> Self {
        Self {
            uow,
            reader,
            destinations,
        }
    }
}
fn minimal(g: &GeographicDescription) -> Option<GeographicDescription> {
    GeographicDescription::new_in_release(
        g.reference_release(),
        None,
        g.country(),
        g.subdivision(),
        g.postal_code().cloned(),
    )
    .expect("validated geography")
}
fn destination_snapshot(facts: DestinationFacts) -> Result<DestinationSnapshot, AssessmentError> {
    let submitted = facts.submitted.as_ref().and_then(minimal);
    let derived = facts.derived.as_ref().and_then(minimal);
    // Submitted assertions take precedence. Conflicting derived facts cannot fill gaps.
    let effective = match (&submitted, &derived) {
        (Some(s), Some(d))
            if s.reference_release() == d.reference_release()
                && s.subdivision()
                    .zip(d.subdivision())
                    .is_none_or(|(s, d)| s == d)
                && s.postal_code()
                    .zip(d.postal_code())
                    .is_none_or(|(s, d)| s == d)
                && s.country()
                    .or_else(|| s.subdivision().map(|v| v.country()))
                    .zip(d.country().or_else(|| d.subdivision().map(|v| v.country())))
                    .is_none_or(|(s, d)| s == d) =>
        {
            GeographicDescription::new_in_release(
                s.reference_release(),
                None,
                s.country().or(d.country()),
                s.subdivision().or(d.subdivision()),
                s.postal_code().or(d.postal_code()).cloned(),
            )
            .map_err(|_| AssessmentError::InvalidInput)?
        }
        (Some(s), _) => Some(s.clone()),
        (None, d) => d.clone(),
    };
    Ok(DestinationSnapshot {
        submitted,
        derived,
        effective,
        attribution: facts.attribution,
    })
}
#[async_trait::async_trait]
impl<U, R> AssessDeliveryUseCase for AssessDeliveryHandler<U, R>
where
    U: UnitOfWork,
    R: AssessmentReaderFactory<U::Tx>,
{
    #[tracing::instrument(name = "shipping.assess_delivery", skip_all, fields(principal_type = context.principal.kind(), request_id = %context.request_id, correlation_id = %context.correlation_id))]
    async fn execute(
        &self,
        context: &OperationContext,
        request: AssessDeliveryRequest,
    ) -> Result<DeliveryAssessmentView, AssessmentError> {
        let ids: Vec<_> = request.listings.iter().map(|i| i.listing_id).collect();
        if ids.is_empty()
            || ids.len() > MAX_ASSESSMENT_LISTINGS
            || ids.iter().enumerate().any(|(i, id)| ids[..i].contains(id))
            || request
                .listings
                .iter()
                .any(|i| i.input.destination.is_some())
        {
            return Err(AssessmentError::InvalidInput);
        }
        let user = match context.principal {
            Principal::User(id) | Principal::DelegatedUser { user_id: id, .. } => Some(id),
            _ => None,
        };
        let destination = match request.destination {
            Some(AssessmentDestination::None) => DestinationFacts {
                submitted: None,
                derived: None,
                attribution: None,
            },
            Some(AssessmentDestination::Geographic(g)) => DestinationFacts {
                submitted: minimal(&g),
                derived: None,
                attribution: None,
            },
            Some(AssessmentDestination::Address(id)) => {
                self.destinations
                    .resolve_destination_facts(context, Some(BrowsingDestination::Address(id)))
                    .await?
            }
            Some(AssessmentDestination::Preference) => {
                self.destinations
                    .resolve_destination_facts(context, None)
                    .await?
            }
            None if user.is_some()
                && context
                    .principal
                    .require_credential_capability(CredentialCapability::UsersRead)
                    .is_ok() =>
            {
                self.destinations
                    .resolve_destination_facts(context, None)
                    .await?
            }
            None => DestinationFacts {
                submitted: None,
                derived: None,
                attribution: None,
            },
        };
        let destination = destination_snapshot(destination)?;
        let mut tx = self.uow.begin().await?;
        let dependencies = self
            .reader
            .in_transaction(&mut tx)
            .snapshot(&ids, user)
            .await?;
        let mut items = Vec::with_capacity(ids.len());
        for mut request in request.listings {
            // Discovery projections can still contain a removed or withdrawn listing.
            // It contributes no current advertised shipping assertion.
            let Some(listing) = dependencies
                .listings
                .iter()
                .find(|l| l.id == request.listing_id)
            else {
                continue;
            };
            let default = dependencies
                .defaults
                .iter()
                .find(|d| d.selection.source_id() == listing.source_id);
            let mut notices = Vec::new();
            if listing.delivery.selection().is_none() {
                notices.push("ASSIGNMENT_UNRESOLVED");
            }
            let listing_expired = listing
                .evidence
                .is_some_and(|e| e.status(dependencies.evaluated_at) == EvidenceStatus::Expired);
            let configuration = if listing_expired || !listing.integration_current {
                notices.push(if listing_expired {
                    "LISTING_EVIDENCE_EXPIRED"
                } else {
                    "INTEGRATION_MISMATCH"
                });
                ListingDeliveryConfiguration::new(
                    listing.source_id,
                    ProfileSelection::SuppressInheritance,
                    Default::default(),
                    Default::default(),
                )
                .expect("empty configuration")
            } else {
                listing.delivery.configuration()
            };
            let profile_id = match configuration.selection() {
                ProfileSelection::Explicit(id) => Some(*id),
                ProfileSelection::InheritSourceDefault => {
                    default.and_then(|d| d.selection.profile_id())
                }
                _ => None,
            };
            let profile = profile_id
                .and_then(|id| dependencies.profiles.iter().find(|p| p.profile.id() == id));
            let profiles: Vec<_> = profile
                .into_iter()
                .filter(|p| {
                    if p.integration != *listing.delivery.integration() {
                        notices.push("PROFILE_INTEGRATION_MISMATCH");
                        false
                    } else if p.evidence.status(dependencies.evaluated_at)
                        == EvidenceStatus::Expired
                    {
                        notices.push("PROFILE_EVIDENCE_EXPIRED");
                        false
                    } else {
                        true
                    }
                })
                .map(|p| p.profile.clone())
                .collect();
            let effective = EffectiveDeliveryTerms::resolve(
                &configuration,
                default.map(|d| &d.selection),
                &profiles,
            );
            let referenced = referenced_locations(effective.terms());
            let locations: Vec<_> = dependencies
                .locations
                .iter()
                .filter(|l| {
                    referenced.contains(&l.location.id())
                        && l.location.party_id() == listing.operator_id
                })
                .map(|l| l.location.clone())
                .collect();
            request.input.destination = destination.effective.clone();
            let assessment = effective.assess(&request.input, &locations);
            items.push(ListingAssessmentView {
                listing_id: listing.id,
                source_id: listing.source_id,
                listing_revision: listing.revision,
                source_revision: listing.source_revision,
                default_revision: matches!(
                    configuration.selection(),
                    ProfileSelection::InheritSourceDefault
                )
                .then(|| default.map_or(1, |d| d.revision)),
                profile: profile.map(|p| ProfileAssessmentBasis {
                    id: p.profile.id(),
                    terms_revision: p.profile.revision().into_inner(),
                    management_revision: p.management_revision,
                    lifecycle: p.profile.lifecycle(),
                    evidence: p.evidence,
                }),
                listing_evidence: listing.evidence,
                evaluated_at: dependencies.evaluated_at,
                input: request.input,
                generic_assessment: effective.assess(&AssessmentInput::default(), &locations),
                effective,
                assessment,
                location_revisions: dependencies
                    .locations
                    .iter()
                    .filter(|l| {
                        referenced.contains(&l.location.id())
                            && l.location.party_id() == listing.operator_id
                    })
                    .map(|l| LocationAssessmentBasis {
                        id: l.location.id(),
                        revision: l.revision,
                        resolution_generation: l.resolution_generation,
                    })
                    .collect(),
                notices,
            });
        }
        tx.commit().await?;
        Ok(DeliveryAssessmentView { destination, items })
    }
}
/// Only explicitly selected sites are dependencies; business/inventory geography is irrelevant.
pub fn referenced_locations(
    terms: &shipping_core::terms::DeliveryTerms,
) -> Vec<party_core::party_location_id::PartyLocationId> {
    use shipping_core::terms::{DeliverySite, ShippingPolicy};
    let origin = match terms.shipping() {
        Some(ShippingPolicy::Coverage(c)) => c.dispatch_origin(),
        _ => None,
    };
    let mut ids = Vec::new();
    for site in origin
        .into_iter()
        .chain(terms.collection().and_then(|c| c.site()))
    {
        if let DeliverySite::PartyLocation(id) = site
            && !ids.contains(id)
        {
            ids.push(*id);
        }
    }
    ids
}

#[cfg(test)]
mod tests {
    use super::*;
    use application::transaction::Transaction;
    use shipping_core::{
        fixtures::gallery,
        selection::{ComponentOverride, DeliveryOverrides},
        terms::ShippingEligibility,
    };
    use std::sync::{
        Mutex,
        atomic::{AtomicUsize, Ordering},
    };
    use user_service::address_book::{
        DestinationView, ResolvedDestination, UserAddressPage, UserAddressView,
    };
    struct Tx(Arc<AtomicUsize>);
    struct Uow(Arc<AtomicUsize>);
    #[async_trait::async_trait]
    impl UnitOfWork for Uow {
        type Tx = Tx;
        async fn begin(&self) -> Result<Tx, TransactionError> {
            Ok(Tx(self.0.clone()))
        }
    }
    #[async_trait::async_trait]
    impl Transaction for Tx {
        async fn commit(self) -> Result<(), TransactionError> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }
    struct Reader(Mutex<Option<AssessmentDependencies>>, AtomicUsize);
    impl AssessmentReaderFactory<Tx> for Reader {
        fn in_transaction<'a>(&'a self, _: &'a mut Tx) -> impl AssessmentReader + 'a {
            Read(self)
        }
    }
    struct Read<'a>(&'a Reader);
    #[async_trait::async_trait]
    impl AssessmentReader for Read<'_> {
        async fn snapshot(
            &mut self,
            ids: &[ProductListingId],
            _: Option<UserId>,
        ) -> Result<AssessmentDependencies, AssessmentError> {
            self.0.1.fetch_add(1, Ordering::SeqCst);
            let deps = self.0.0.lock().unwrap().take().unwrap();
            assert_eq!(ids.len(), deps.listings.len());
            Ok(deps)
        }
    }
    struct Destinations(AtomicUsize);
    #[async_trait::async_trait]
    impl AddressBookQueries for Destinations {
        async fn get(
            &self,
            _: &OperationContext,
            _: UserAddressId,
        ) -> Result<UserAddressView, AddressBookError> {
            unreachable!()
        }
        async fn list(
            &self,
            _: &OperationContext,
            _: Option<UserAddressId>,
            _: u32,
        ) -> Result<UserAddressPage, AddressBookError> {
            unreachable!()
        }
        async fn destination(
            &self,
            _: &OperationContext,
        ) -> Result<DestinationView, AddressBookError> {
            unreachable!()
        }
        async fn resolve_destination(
            &self,
            ctx: &OperationContext,
            explicit: Option<BrowsingDestination>,
        ) -> Result<ResolvedDestination, AddressBookError> {
            self.0.fetch_add(1, Ordering::SeqCst);
            if matches!(ctx.principal, Principal::Anonymous) {
                return Err(AddressBookError::AuthenticationRequired);
            }
            if matches!(explicit, Some(BrowsingDestination::Address(_))) {
                return Err(AddressBookError::NotFound);
            }
            Ok(ResolvedDestination {
                destination: BrowsingDestination::None,
                submitted: gallery().assessment_input().destination,
                derived: None,
                resolution_status: None,
                attribution: None,
            })
        }
    }
    fn context(principal: Principal) -> OperationContext {
        OperationContext {
            principal,
            request_id: "assessment".into(),
            correlation_id: "assessment".into(),
        }
    }
    fn dependencies() -> AssessmentDependencies {
        let g = gallery();
        let at = OffsetDateTime::UNIX_EPOCH + time::Duration::days(100);
        let evidence = AssertionEvidence {
            verified_at: at - time::Duration::days(1),
            fresh_until: None,
            expires_at: None,
        };
        let listings = [g.painting, g.cabinet]
            .into_iter()
            .map(|c| ListingDependency {
                id: ProductListingId::new(),
                source_id: g.source_id,
                revision: 1,
                source_revision: 1,
                operator_id: g.party_id,
                delivery: ListingDelivery::new(
                    g.source_id,
                    Default::default(),
                    Some(c.selection().clone()),
                    c.rate_inputs().clone(),
                    c.overrides().clone(),
                )
                .unwrap(),
                evidence: Some(evidence),
                integration_current: true,
            })
            .collect();
        AssessmentDependencies {
            evaluated_at: at,
            listings,
            profiles: vec![ProfileDependency {
                profile: g.profile,
                management_revision: 1,
                integration: Default::default(),
                evidence,
            }],
            defaults: vec![DefaultDependency {
                selection: g.source_default,
                revision: 2,
            }],
            locations: vec![],
        }
    }
    fn request(
        deps: &AssessmentDependencies,
        destination: Option<AssessmentDestination>,
    ) -> AssessDeliveryRequest {
        AssessDeliveryRequest {
            destination,
            listings: deps
                .listings
                .iter()
                .map(|l| AssessListingRequest {
                    listing_id: l.id,
                    input: AssessmentInput {
                        quantity: std::num::NonZeroU64::new(1),
                        ..Default::default()
                    },
                })
                .collect(),
        }
    }
    fn handler(deps: AssessmentDependencies) -> AssessDeliveryHandler<Uow, Reader> {
        AssessDeliveryHandler::new(
            Uow(Arc::new(AtomicUsize::new(0))),
            Reader(Mutex::new(Some(deps)), AtomicUsize::new(0)),
            Arc::new(Destinations(AtomicUsize::new(0))),
        )
    }
    #[tokio::test]
    async fn anonymous_explicit_input_batches_shared_profile_and_commits_once() {
        let deps = dependencies();
        let request = request(
            &deps,
            Some(AssessmentDestination::Geographic(
                gallery().assessment_input().destination.unwrap(),
            )),
        );
        let handler = handler(deps);
        let view = handler
            .execute(&context(Principal::Anonymous), request)
            .await
            .unwrap();
        assert_eq!(
            Some(ShippingEligibility::Offered),
            view.items[0].assessment.eligibility
        );
        assert_eq!(
            Some(ShippingEligibility::NotOffered),
            view.items[1].assessment.eligibility
        );
        assert_eq!(
            Some(ShippingEligibility::Offered),
            view.items[1].assessment.collection.eligibility
        );
        assert_eq!(1, handler.reader.1.load(Ordering::SeqCst));
        assert_eq!(1, handler.uow.0.load(Ordering::SeqCst));
    }
    #[tokio::test]
    async fn a_hundred_listing_page_resolves_one_destination_and_one_shared_snapshot() {
        let mut deps = dependencies();
        let template = deps.listings[0].clone();
        deps.listings = (0..MAX_ASSESSMENT_LISTINGS)
            .map(|_| ListingDependency {
                id: ProductListingId::new(),
                ..template.clone()
            })
            .collect();
        let request = request(&deps, None);
        let expected_ids = request
            .listings
            .iter()
            .map(|r| r.listing_id)
            .collect::<Vec<_>>();
        let destinations = Arc::new(Destinations(AtomicUsize::new(0)));
        let handler = AssessDeliveryHandler::new(
            Uow(Arc::new(AtomicUsize::new(0))),
            Reader(Mutex::new(Some(deps)), AtomicUsize::new(0)),
            destinations.clone(),
        );
        let view = handler
            .execute(&context(Principal::User(UserId::new())), request)
            .await
            .unwrap();
        assert_eq!(
            expected_ids,
            view.items.iter().map(|v| v.listing_id).collect::<Vec<_>>()
        );
        assert!(
            view.items
                .iter()
                .all(|v| v.assessment.eligibility == Some(ShippingEligibility::Offered))
        );
        assert_eq!(1, destinations.0.load(Ordering::SeqCst));
        assert_eq!(1, handler.reader.1.load(Ordering::SeqCst));
        assert_eq!(1, handler.uow.0.load(Ordering::SeqCst));
    }
    #[tokio::test]
    async fn absence_uses_owned_preference_and_explicit_none_overrides_it() {
        for (principal, destination, offered) in [
            (Principal::Anonymous, None, false),
            (Principal::User(UserId::new()), None, true),
            (
                Principal::User(UserId::new()),
                Some(AssessmentDestination::None),
                false,
            ),
        ] {
            let deps = dependencies();
            let request = request(&deps, destination);
            let handler = handler(deps);
            let view = handler.execute(&context(principal), request).await.unwrap();
            assert_eq!(
                offered,
                view.items[0].assessment.eligibility == Some(ShippingEligibility::Offered)
            );
        }
    }
    #[tokio::test]
    async fn address_errors_and_anonymous_preferences_never_fall_back() {
        for (principal, destination) in [
            (
                Principal::User(UserId::new()),
                AssessmentDestination::Address(UserAddressId::new()),
            ),
            (Principal::Anonymous, AssessmentDestination::Preference),
        ] {
            let deps = dependencies();
            let request = request(&deps, Some(destination));
            let handler = handler(deps);
            assert!(matches!(
                handler.execute(&context(principal), request).await,
                Err(AssessmentError::Destination(_))
            ));
            assert_eq!(0, handler.reader.1.load(Ordering::SeqCst));
        }
    }
    #[tokio::test]
    async fn stale_evidence_keeps_age_and_expiry_only_removes_inherited_assertions() {
        for expired in [false, true] {
            let mut deps = dependencies();
            let at = deps.evaluated_at;
            deps.profiles[0].evidence.fresh_until = Some(at);
            deps.profiles[0].evidence.expires_at =
                Some(at + time::Duration::seconds(if expired { 0 } else { 1 }));
            let request = request(
                &deps,
                Some(AssessmentDestination::Geographic(
                    gallery().assessment_input().destination.unwrap(),
                )),
            );
            let view = handler(deps)
                .execute(&context(Principal::Anonymous), request)
                .await
                .unwrap();
            assert_eq!(
                !expired,
                view.items[0].assessment.eligibility == Some(ShippingEligibility::Offered)
            );
            assert_eq!(
                Some(ShippingEligibility::NotOffered),
                view.items[1].assessment.eligibility
            );
            assert_eq!(
                if expired {
                    None
                } else {
                    Some(ShippingEligibility::Offered)
                },
                view.items[1].assessment.collection.eligibility
            );
            assert_eq!(
                at - time::Duration::days(1),
                view.items[0].profile.as_ref().unwrap().evidence.verified_at
            );
        }
    }
    #[tokio::test]
    async fn expired_inline_assertion_and_unresolved_provider_never_offer_default() {
        let mut deps = dependencies();
        let at = deps.evaluated_at;
        deps.listings[0].evidence.as_mut().unwrap().expires_at = Some(at);
        deps.listings[1].delivery = ListingDelivery::new(
            deps.listings[1].source_id,
            Default::default(),
            None,
            Default::default(),
            DeliveryOverrides {
                shipping: ComponentOverride::Inherit,
                collection: ComponentOverride::Inherit,
            },
        )
        .unwrap();
        let request = request(
            &deps,
            Some(AssessmentDestination::Geographic(
                gallery().assessment_input().destination.unwrap(),
            )),
        );
        let view = handler(deps)
            .execute(&context(Principal::Anonymous), request)
            .await
            .unwrap();
        assert!(
            view.items
                .iter()
                .all(|i| i.assessment.eligibility.is_none())
        );
        assert_eq!(vec!["ASSIGNMENT_UNRESOLVED"], view.items[1].notices);
    }
    #[test]
    fn destination_snapshot_strips_street_and_keeps_submitted_separate_from_derived() {
        let submitted = GeographicDescription::new(
            Some(geo::AddressText::new("private street").unwrap()),
            Some(geo::CountryCode::DEU),
            None,
            None,
        )
        .unwrap();
        let derived = GeographicDescription::new(
            None,
            Some(geo::CountryCode::FRA),
            None,
            Some(geo::PostalCode::new("75001").unwrap()),
        )
        .unwrap();
        let snapshot = destination_snapshot(DestinationFacts {
            submitted,
            derived,
            attribution: None,
        })
        .unwrap();
        assert!(
            snapshot
                .submitted
                .as_ref()
                .unwrap()
                .address_text()
                .is_none()
        );
        assert_eq!(
            Some(geo::CountryCode::DEU),
            snapshot.effective.as_ref().unwrap().country()
        );
        assert!(snapshot.effective.unwrap().postal_code().is_none());
    }
    #[test]
    fn conflicting_derived_postcode_cannot_supply_a_subdivision() {
        let submitted = GeographicDescription::new(
            None,
            Some(geo::CountryCode::DEU),
            None,
            Some(geo::PostalCode::new("10115").unwrap()),
        )
        .unwrap();
        let derived = GeographicDescription::new(
            None,
            Some(geo::CountryCode::DEU),
            Some(
                geo::SubdivisionCode::from_code("DE-BY", geo::reference::ReferenceRelease::CURRENT)
                    .unwrap(),
            ),
            Some(geo::PostalCode::new("80331").unwrap()),
        )
        .unwrap();
        let snapshot = destination_snapshot(DestinationFacts {
            submitted: submitted.clone(),
            derived: derived.clone(),
            attribution: None,
        })
        .unwrap();
        assert_eq!(submitted, snapshot.effective);
        assert_eq!(derived, snapshot.derived);
    }
    #[tokio::test]
    async fn foreign_locations_cannot_establish_eligibility_or_expose_revision_metadata() {
        use party_core::{party_location::*, party_location_id::PartyLocationId};
        use shipping_core::{coverage::ShippingCoverage, terms::*};
        let mut deps = dependencies();
        let site = PartyLocationId::new();
        let terms = &deps.profiles[0].profile.content().terms;
        let ShippingPolicy::Coverage(coverage) = terms.shipping().unwrap() else {
            unreachable!()
        };
        let terms = DeliveryTerms::new(
            Some(ShippingPolicy::coverage(
                ShippingCoverage::new(
                    coverage.release(),
                    coverage.completeness(),
                    coverage.applicability().clone(),
                    coverage.exclusions().to_vec(),
                    coverage.zones().to_vec(),
                    Some(DeliverySite::PartyLocation(site)),
                )
                .unwrap(),
            )),
            None,
        );
        deps.listings[0].delivery = ListingDelivery::new(
            deps.listings[0].source_id,
            Default::default(),
            Some(ProfileSelection::inline(terms)),
            Default::default(),
            Default::default(),
        )
        .unwrap();
        deps.locations.push(LocationDependency {
            location: PartyLocation::create(
                site,
                party_core::party_id::PartyId::new(),
                PartyLocationContent {
                    label: PartyLocationLabel::new("Foreign site".into()).unwrap(),
                    roles: Default::default(),
                    geography: None,
                    position: None,
                },
            ),
            revision: 1,
            resolution_generation: None,
        });
        let request = request(
            &deps,
            Some(AssessmentDestination::Geographic(
                gallery().assessment_input().destination.unwrap(),
            )),
        );
        let view = handler(deps)
            .execute(&context(Principal::Anonymous), request)
            .await
            .unwrap();
        assert!(view.items[0].assessment.eligibility.is_none());
        assert_eq!(
            vec![shipping_core::assessment::AssessmentReason::LocationAbsent(
                site
            )],
            view.items[0].assessment.reasons
        );
        assert!(view.items[0].location_revisions.is_empty());
    }
    #[tokio::test]
    async fn invalid_batches_are_rejected_before_private_or_storage_reads() {
        for count in [0, 21] {
            let deps = dependencies();
            let id = deps.listings[0].id;
            let request = AssessDeliveryRequest {
                destination: None,
                listings: (0..count)
                    .map(|_| AssessListingRequest {
                        listing_id: id,
                        input: Default::default(),
                    })
                    .collect(),
            };
            let handler = handler(deps);
            assert!(matches!(
                handler
                    .execute(&context(Principal::User(UserId::new())), request)
                    .await,
                Err(AssessmentError::InvalidInput)
            ));
            assert_eq!(0, handler.reader.1.load(Ordering::SeqCst));
        }
    }
}

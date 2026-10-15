//! Use-case orchestration over the existing profile and ProductListing aggregates.
use crate::{ports::*, *};
use product_listing_core::{
    delivery::ListingDelivery, listing_lifecycle::ListingLifecycle,
    product_listing_id::ProductListingKey,
};
use product_listing_service::{
    ports::{
        ProductListingEventAppender, ProductListingEventAppenderFactory, ProductListingRepository,
        ProductListingRepositoryFactory, ProductListingWriteEffects,
        product_listing_delivery::{
            ProductListingDeliveryReferences, ProductListingDeliveryReferencesFactory,
        },
        stamp_product_listing_event,
    },
    use_cases::ProductListingDeliveryError,
};
use shipping_core::{
    inputs::ListingRateInputs,
    profile::ShippingProfile,
    selection::{ComponentOverride, DeliveryOverrides},
};
use shipping_service::ports::{
    ShippingManagementRevision, ShippingRepository, ShippingRepositoryFactory, StoredProfile,
};

pub struct CanonicalShippingSync<S, P, L, E, V> {
    sync: S,
    profiles: P,
    listings: L,
    events: E,
    references: V,
}
impl<S, P, L, E, V> CanonicalShippingSync<S, P, L, E, V> {
    pub fn new(sync: S, profiles: P, listings: L, events: E, references: V) -> Self {
        Self {
            sync,
            profiles,
            listings,
            events,
            references,
        }
    }
}
fn actor(binding: &Binding) -> String {
    format!("shipping-sync:{}", binding.epoch)
}
fn revision_next(value: u64, amount: u64) -> Result<u64, SyncError> {
    value
        .checked_add(amount)
        .filter(|n| *n <= i64::MAX as u64)
        .ok_or_else(|| shipping_service::ShippingError::RevisionExhausted.into())
}

impl<S, P, L, E, V> CanonicalShippingSync<S, P, L, E, V> {
    async fn profile<Tx>(
        &self,
        tx: &mut Tx,
        binding: &Binding,
        record: &Record,
        previous: Option<&Claim>,
        content: &ShippingProfileContent,
        evidence: &[ShippingEvidence],
    ) -> Result<(AppliedValue, bool), SyncError>
    where
        Tx: Send,
        S: SyncStoreFactory<Tx>,
        P: ShippingRepositoryFactory<Tx>,
        V: ProductListingDeliveryReferencesFactory<Tx>,
    {
        let mut stored = if let Some(Claim {
            value: AppliedValue::Profile { id, revision },
            ..
        }) = previous
        {
            let current = self
                .profiles
                .in_transaction(tx)
                .find_for_update(binding.source, *id)
                .await?;
            if current.management_revision.into_inner() != *revision {
                return Ok((previous.expect("previous claim").value.clone(), false));
            }
            current
        } else {
            let stored = StoredProfile {
                profile: ShippingProfile::create(
                    ShippingProfileId::new(),
                    binding.source,
                    content.clone(),
                )
                .map_err(shipping_service::ShippingError::from)?,
                management_revision: ShippingManagementRevision::INITIAL,
                evidence: evidence.to_vec(),
            };
            use sha2::{Digest, Sha256};
            let key = Sha256::digest(record.key.as_str().as_bytes())
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect::<String>();
            self.profiles
                .in_transaction(tx)
                .insert(&stored, &actor(binding), &key)
                .await?;
            self.sync
                .in_transaction(tx)
                .mark_profile(binding, stored.profile.id(), record.window, true)
                .await?;
            self.references
                .in_transaction(tx)
                .validate(
                    &ListingDelivery::new(
                        binding.source,
                        binding.integration.clone(),
                        Some(ProfileSelection::Explicit(stored.profile.id())),
                        ListingRateInputs::default(),
                        DeliveryOverrides::default(),
                    )
                    .map_err(shipping_service::ShippingError::from)?,
                )
                .await?;
            return Ok((
                AppliedValue::Profile {
                    id: stored.profile.id(),
                    revision: 1,
                },
                true,
            ));
        };
        let expected = stored.management_revision;
        let old_terms = stored.profile.content().terms.clone();
        let old_revision = stored.profile.revision().into_inner();
        let restored = stored
            .profile
            .restore()
            .map_err(shipping_service::ShippingError::from)?
            .changed();
        let changed = stored
            .profile
            .replace_content(content.clone())
            .map_err(shipping_service::ShippingError::from)?
            .changed();
        if changed || restored || stored.evidence != evidence {
            stored.evidence = evidence.to_vec();
            let increment = (stored.profile.revision().into_inner() - old_revision).max(1);
            stored.management_revision = ShippingManagementRevision::try_from(revision_next(
                expected.into_inner(),
                increment,
            )?)
            .map_err(failure)?;
            self.profiles
                .in_transaction(tx)
                .save(&stored, expected, &actor(binding))
                .await?;
            self.sync
                .in_transaction(tx)
                .mark_profile(
                    binding,
                    stored.profile.id(),
                    record.window,
                    restored || old_terms != content.terms,
                )
                .await?;
        }
        self.references
            .in_transaction(tx)
            .validate(
                &ListingDelivery::new(
                    binding.source,
                    binding.integration.clone(),
                    Some(ProfileSelection::Explicit(stored.profile.id())),
                    ListingRateInputs::default(),
                    DeliveryOverrides::default(),
                )
                .map_err(shipping_service::ShippingError::from)?,
            )
            .await?;
        Ok((
            AppliedValue::Profile {
                id: stored.profile.id(),
                revision: stored.management_revision.into_inner(),
            },
            restored || old_terms != content.terms,
        ))
    }
    async fn profile_id<Tx>(
        &self,
        tx: &mut Tx,
        binding: &Binding,
        key: &ExternalKey,
    ) -> Result<ShippingProfileId, SyncError>
    where
        Tx: Send,
        S: SyncStoreFactory<Tx>,
    {
        match self
            .sync
            .in_transaction(tx)
            .claim(binding, ObjectKind::Profile, key)
            .await?
        {
            Some(Claim {
                value: AppliedValue::Profile { id, .. },
                tombstoned: false,
                ..
            }) => Ok(id),
            _ => Err(SyncError::Dependency("PROFILE_MISSING")),
        }
    }
    async fn default<Tx>(
        &self,
        tx: &mut Tx,
        binding: &Binding,
        previous: Option<&Claim>,
        profile: Option<ShippingProfileId>,
        window: ObservationWindow,
    ) -> Result<(AppliedValue, bool), SyncError>
    where
        Tx: Send,
        S: SyncStoreFactory<Tx>,
        P: ShippingRepositoryFactory<Tx>,
        V: ProductListingDeliveryReferencesFactory<Tx>,
    {
        let mut current = self
            .profiles
            .in_transaction(tx)
            .default_for_update(binding.source)
            .await?;
        let owner = self.sync.in_transaction(tx).default_owner(binding).await?;
        let owned = match previous.map(|p| &p.value) {
            Some(AppliedValue::Default { revision, .. }) => {
                current.revision.into_inner() == *revision
                    && (owner == Some(binding.epoch) || *revision == 1)
            }
            _ => current.revision.into_inner() == 1 || owner.is_some_and(|e| e != binding.epoch),
        };
        if !owned {
            return Ok((
                previous
                    .map(|p| p.value.clone())
                    .unwrap_or(AppliedValue::Tombstone),
                false,
            ));
        }
        let changed = if let Some(id) = profile {
            let p = self
                .profiles
                .in_transaction(tx)
                .find_for_update(binding.source, id)
                .await?;
            self.references
                .in_transaction(tx)
                .validate(
                    &ListingDelivery::new(
                        binding.source,
                        binding.integration.clone(),
                        Some(ProfileSelection::Explicit(id)),
                        ListingRateInputs::default(),
                        DeliveryOverrides::default(),
                    )
                    .map_err(shipping_service::ShippingError::from)?,
                )
                .await?;
            current
                .selection
                .select(Some(&p.profile))
                .map_err(shipping_service::ShippingError::from)?
                .changed()
        } else {
            current
                .selection
                .select(None)
                .map_err(shipping_service::ShippingError::from)?
                .changed()
        };
        if changed {
            let expected = current.revision;
            current.revision = revision_next(expected.into_inner(), 1)?
                .try_into()
                .map_err(failure)?;
            self.profiles
                .in_transaction(tx)
                .save_default(&current, expected, &actor(binding))
                .await?;
        }
        self.sync
            .in_transaction(tx)
            .mark_default(binding, window, changed)
            .await?;
        Ok((
            AppliedValue::Default {
                revision: current.revision.into_inner(),
                profile: current.selection.profile_id(),
            },
            changed,
        ))
    }
    async fn item<Tx>(
        &self,
        tx: &mut Tx,
        binding: &Binding,
        record: &Record,
        previous: Option<&Claim>,
        source_listing_id: &SourceListingId,
        incoming: Option<&ItemAssertion>,
    ) -> Result<AppliedValue, SyncError>
    where
        Tx: Send,
        S: SyncStoreFactory<Tx>,
        L: ProductListingRepositoryFactory<Tx>,
        E: ProductListingEventAppenderFactory<Tx>,
        V: ProductListingDeliveryReferencesFactory<Tx>,
    {
        let key = ProductListingKey::new(binding.source, source_listing_id.clone());
        match self.references.in_transaction(tx).lock_target(&key).await {
            Err(ProductListingDeliveryError::NotFound) => {
                return Err(SyncError::Dependency("LISTING_MISSING"));
            }
            r => r?,
        }
        let listing = self
            .listings
            .in_transaction(tx)
            .find_by_key(&key)
            .await
            .map_err(failure)?
            .ok_or(SyncError::Dependency("LISTING_MISSING"))?;
        if listing.value.lifecycle() != ListingLifecycle::Active {
            return Err(SyncError::Dependency("LISTING_WITHDRAWN"));
        }
        // Missing extraction is distinct from a reliable unresolved assignment. Preserve
        // existing assertions and their age; first provider observation can still install
        // an unresolved blocker so it never affirms a source default accidentally.
        if incoming.is_some_and(|i| i.selection == EvidenceSelection::Unavailable)
            && let Some(current) = listing.value.delivery()
        {
            if current.integration() != &binding.integration {
                return Err(SyncError::Dependency("INTEGRATION_CONFLICT"));
            }
            return Ok(previous
                .map(|c| c.value.clone())
                .unwrap_or_else(|| AppliedValue::Item {
                    source_listing_id: source_listing_id.clone(),
                    delivery: Box::new(current.clone()),
                    owned: [false; 4],
                }));
        }
        let initial = if binding.integration == DeliveryIntegration::Source {
            ListingDelivery::inherited(binding.source)
        } else {
            ListingDelivery::new(
                binding.source,
                binding.integration.clone(),
                None,
                ListingRateInputs::default(),
                DeliveryOverrides::default(),
            )
            .map_err(shipping_service::ShippingError::from)?
        };
        let current = listing.value.delivery().unwrap_or(&initial);
        let defaults = ListingRateInputs::default();
        let inherited = DeliveryOverrides::default();
        let ownership = self
            .sync
            .in_transaction(tx)
            .item_ownership(binding, source_listing_id)
            .await?;
        let mut owned = ownership.reclaimable;
        if previous.is_some_and(|p| p.tombstoned) {
            for (i, value) in owned.iter_mut().enumerate() {
                *value |= ownership.current[i];
            }
        }
        let reclaimed = owned;
        let integration_changed = current.integration() != &binding.integration;
        if integration_changed
            && !((current.selection().is_none() || owned[0])
                && (current.rate_inputs() == &defaults || owned[1])
                && (current.overrides().shipping == inherited.shipping || owned[2])
                && (current.overrides().collection == inherited.collection || owned[3]))
        {
            return Err(SyncError::Dependency("INTEGRATION_CONFLICT"));
        }
        let current = if integration_changed {
            &initial
        } else {
            current
        };
        if let Some(Claim {
            value:
                AppliedValue::Item {
                    source_listing_id: old_key,
                    delivery,
                    owned: old_owned,
                },
            ..
        }) = previous
        {
            if old_key != source_listing_id {
                return Err(SyncError::Conflict);
            }
            owned[0] |=
                old_owned[0] && ownership.current[0] && delivery.selection() == current.selection();
            owned[1] |= old_owned[1]
                && ownership.current[1]
                && delivery.rate_inputs() == current.rate_inputs();
            owned[2] |= old_owned[2]
                && ownership.current[2]
                && delivery.overrides().shipping == current.overrides().shipping;
            owned[3] |= old_owned[3]
                && ownership.current[3]
                && delivery.overrides().collection == current.overrides().collection;
        } else {
            owned[0] |= listing.value.delivery().is_none() || current.selection().is_none();
            owned[1] |= current.rate_inputs() == &defaults;
            owned[2] |= current.overrides().shipping == inherited.shipping;
            owned[3] |= current.overrides().collection == inherited.collection;
        }
        let selection = if owned[0] {
            match incoming.map(|i| &i.selection) {
                Some(s) => {
                    let id = match s {
                        EvidenceSelection::Profile(k) => {
                            Some(self.profile_id(tx, binding, k).await?)
                        }
                        _ => None,
                    };
                    selected(s, id)
                }
                None => None,
            }
        } else {
            current.selection().cloned()
        };
        let delivery = ListingDelivery::new(
            binding.source,
            binding.integration.clone(),
            selection,
            if owned[1] {
                incoming.map(|i| i.rate_inputs.clone()).unwrap_or_default()
            } else {
                current.rate_inputs().clone()
            },
            DeliveryOverrides {
                shipping: if owned[2] {
                    incoming
                        .map(|i| i.overrides.shipping.clone())
                        .unwrap_or(ComponentOverride::Inherit)
                } else {
                    current.overrides().shipping.clone()
                },
                collection: if owned[3] {
                    incoming
                        .map(|i| i.overrides.collection.clone())
                        .unwrap_or(ComponentOverride::Inherit)
                } else {
                    current.overrides().collection.clone()
                },
            },
        )
        .map_err(shipping_service::ShippingError::from)?;
        let changed = [
            (listing.value.delivery().is_none()
                && binding.integration != DeliveryIntegration::Source)
                || delivery.selection() != current.selection(),
            delivery.rate_inputs() != current.rate_inputs(),
            delivery.overrides().shipping != current.overrides().shipping,
            delivery.overrides().collection != current.overrides().collection,
        ];
        self.references
            .in_transaction(tx)
            .validate(&delivery)
            .await?;
        let mut aggregate = listing.value;
        aggregate
            .replace_delivery(Some(delivery.clone()))
            .map_err(failure)?;
        if let Some(payload) = aggregate.take_pending_event_payload() {
            let event = stamp_product_listing_event(
                aggregate.id(),
                time::OffsetDateTime::now_utc(),
                payload,
            );
            let effects = ProductListingWriteEffects::from(&event.payload);
            self.listings
                .in_transaction(tx)
                .update(&aggregate, listing.version, event.event_id, effects)
                .await
                .map_err(failure)?;
            self.events
                .in_transaction(tx)
                .append(&event)
                .await
                .map_err(failure)?;
        }
        // First ownership needs an acquisition window even when its value equals the
        // aggregate's implicit defaults. Subsequent equal observations retain that age.
        let changed = std::array::from_fn(|i| {
            changed[i] || (owned[i] && !ownership.current[i]) || reclaimed[i] || integration_changed
        });
        self.sync
            .in_transaction(tx)
            .mark_item(
                binding,
                &delivery,
                source_listing_id,
                owned,
                record.window,
                changed,
            )
            .await?;
        Ok(AppliedValue::Item {
            source_listing_id: source_listing_id.clone(),
            delivery: Box::new(delivery),
            owned,
        })
    }
}

#[async_trait::async_trait]
impl<Tx, S, P, L, E, V> SyncApplication<Tx> for CanonicalShippingSync<S, P, L, E, V>
where
    Tx: Send,
    S: SyncStoreFactory<Tx>,
    P: ShippingRepositoryFactory<Tx>,
    L: ProductListingRepositoryFactory<Tx>,
    E: ProductListingEventAppenderFactory<Tx>,
    V: ProductListingDeliveryReferencesFactory<Tx>,
{
    async fn apply(
        &self,
        tx: &mut Tx,
        binding: &Binding,
        receipt: &Receipt,
        records: &[Record],
    ) -> Result<bool, SyncError> {
        // Profiles precede assignments regardless of chunk delivery order. Configuration is
        // bounded; no product fact or lifecycle is created/restored by this use case.
        let mut configuration_changed = false;
        let mut ordered = records.iter().collect::<Vec<_>>();
        ordered.sort_by_key(|r| match r.assertion {
            Assertion::Profile { .. } => 0,
            Assertion::Default { .. } => 1,
            Assertion::Item { .. } => 2,
            _ => 3,
        });
        let old = self
            .sync
            .in_transaction(tx)
            .claims(binding, &receipt.begin.scope)
            .await?;
        for record in ordered {
            let Some(kind) = record.kind() else { continue };
            let previous = self
                .sync
                .in_transaction(tx)
                .claim(binding, kind, &record.key)
                .await?;
            if let Some(p) = &previous {
                if p.scope != receipt.begin.scope {
                    return Err(SyncError::Conflict);
                }
                if record.window.observed_at < p.window.observed_at {
                    return Err(SyncError::Dirty);
                }
            }
            if previous.is_some()
                && matches!(&record.assertion,Assertion::Item{assertion,..} if assertion.selection==EvidenceSelection::Unavailable)
            {
                continue;
            }
            let (value, changed) = match &record.assertion {
                Assertion::Profile { content, evidence } => {
                    self.profile(tx, binding, record, previous.as_ref(), content, evidence)
                        .await?
                }
                Assertion::Default { profile } => {
                    if record.key.as_str() != "default" {
                        return Err(SyncError::InvalidInput);
                    }
                    let profile = match profile {
                        Some(k) => Some(self.profile_id(tx, binding, k).await?),
                        None => None,
                    };
                    self.default(tx, binding, previous.as_ref(), profile, record.window)
                        .await?
                }
                Assertion::Item {
                    source_listing_id,
                    assertion,
                } => {
                    if source_listing_id.as_ref() != record.key.as_str() {
                        return Err(SyncError::InvalidInput);
                    }
                    (
                        self.item(
                            tx,
                            binding,
                            record,
                            previous.as_ref(),
                            source_listing_id,
                            Some(assertion),
                        )
                        .await?,
                        false,
                    )
                }
                Assertion::Delete { .. } => {
                    self.retract(tx, binding, record, previous.as_ref()).await?
                }
                Assertion::Unavailable { .. } => continue,
            };
            configuration_changed |= changed;
            let unchanged = previous.as_ref().is_some_and(|p| p.value == value);
            let claim = Claim {
                kind,
                key: record.key.clone(),
                scope: receipt.begin.scope.clone(),
                window: if unchanged {
                    previous.as_ref().expect("previous").window
                } else {
                    record.window
                },
                value,
                tombstoned: matches!(record.assertion, Assertion::Delete { .. }),
            };
            self.sync
                .in_transaction(tx)
                .save_claim(binding, &claim)
                .await?;
        }
        if receipt.begin.mode == Mode::Snapshot {
            for old in old.into_iter().filter(|c| {
                !c.tombstoned
                    && receipt.begin.components[match c.kind {
                        ObjectKind::Profile => 0,
                        ObjectKind::Default => 1,
                        ObjectKind::Item => 2,
                    }]
                    && !records
                        .iter()
                        .any(|r| r.kind() == Some(c.kind) && r.key == c.key)
            }) {
                let record = Record {
                    key: old.key.clone(),
                    window: old.window,
                    assertion: Assertion::Delete { kind: old.kind },
                    objects: vec![],
                };
                let (value, changed) = self.retract(tx, binding, &record, Some(&old)).await?;
                configuration_changed |= changed;
                self.sync
                    .in_transaction(tx)
                    .save_claim(
                        binding,
                        &Claim {
                            value,
                            tombstoned: true,
                            ..old
                        },
                    )
                    .await?;
            }
        }
        Ok(configuration_changed)
    }
}
impl<S, P, L, E, V> CanonicalShippingSync<S, P, L, E, V> {
    async fn retract<Tx>(
        &self,
        tx: &mut Tx,
        binding: &Binding,
        record: &Record,
        previous: Option<&Claim>,
    ) -> Result<(AppliedValue, bool), SyncError>
    where
        Tx: Send,
        S: SyncStoreFactory<Tx>,
        P: ShippingRepositoryFactory<Tx>,
        L: ProductListingRepositoryFactory<Tx>,
        E: ProductListingEventAppenderFactory<Tx>,
        V: ProductListingDeliveryReferencesFactory<Tx>,
    {
        match previous.map(|p| &p.value) {
            Some(AppliedValue::Profile { id, revision }) => {
                let mut p = self
                    .profiles
                    .in_transaction(tx)
                    .find_for_update(binding.source, *id)
                    .await?;
                if p.management_revision.into_inner() == *revision
                    && p.profile
                        .retire()
                        .map_err(shipping_service::ShippingError::from)?
                        .changed()
                {
                    let expected = p.management_revision;
                    p.management_revision = revision_next(expected.into_inner(), 1)?
                        .try_into()
                        .map_err(failure)?;
                    self.profiles
                        .in_transaction(tx)
                        .save(&p, expected, &actor(binding))
                        .await?;
                    return Ok((
                        AppliedValue::Profile {
                            id: *id,
                            revision: p.management_revision.into_inner(),
                        },
                        true,
                    ));
                }
                Ok((previous.expect("previous").value.clone(), false))
            }
            Some(AppliedValue::Default { .. }) => {
                self.default(tx, binding, previous, None, record.window)
                    .await
            }
            Some(AppliedValue::Item {
                source_listing_id, ..
            }) => match self
                .item(tx, binding, record, previous, source_listing_id, None)
                .await
            {
                Ok(value) => Ok((value, false)),
                Err(SyncError::Dependency("LISTING_MISSING" | "LISTING_WITHDRAWN")) => {
                    Ok((AppliedValue::Tombstone, false))
                }
                Err(error) => Err(error),
            },
            _ => Ok((AppliedValue::Tombstone, false)),
        }
    }
}

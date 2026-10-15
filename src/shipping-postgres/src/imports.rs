//! Transaction-bound import identity, ownership and validity persistence.
use super::{corrupt, invalid, sql_error, terms};
use application::{operation_context::OperationContext, patch_field::PatchField};
use listing_source_core::ListingSourceId;
use platform_postgres::SqlxTransaction;
use product_listing_core::delivery::DeliveryIntegration;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use shipping_core::{
    inputs::ListingRateInputs,
    profile::ShippingProfileId,
    selection::{ComponentOverride, ProfileSelection},
};
use shipping_json::listing_delivery as codec;
use shipping_service::{ShippingError, imports::*};
use sqlx::{FromRow, PgConnection};
use time::OffsetDateTime;
use uuid::Uuid;

#[derive(Clone, Copy, Default)]
pub struct SqlxShippingImportAccessFactory;
pub(super) struct Repository<'a>(pub(super) &'a mut PgConnection);
impl ShippingImportAccessFactory<SqlxTransaction> for SqlxShippingImportAccessFactory {
    fn in_transaction<'a>(&'a self, tx: &'a mut SqlxTransaction) -> impl ShippingImportAccess + 'a {
        Repository(tx.connection())
    }
}
fn hash(bytes: impl AsRef<[u8]>) -> Vec<u8> {
    Sha256::digest(bytes).to_vec()
}
fn encoded(result: Result<Value, application::error::BoxError>) -> Result<Value, ShippingError> {
    result.map_err(super::encode_error)
}
fn patch<T>(
    p: &PatchField<T>,
    f: impl FnOnce(&T) -> Result<Value, application::error::BoxError>,
) -> Result<Value, ShippingError> {
    Ok(match p {
        PatchField::Unchanged => json!({"state":"UNCHANGED"}),
        PatchField::Clear => json!({"state":"CLEAR"}),
        PatchField::Set(v) => json!({"state":"SET","value":encoded(f(v))?}),
    })
}
fn content(
    c: &shipping_core::profile::ShippingProfileContent,
    evidence: &[shipping_service::ports::ShippingEvidence],
) -> Result<Value, ShippingError> {
    Ok(json!([
        c.name.as_str(),
        c.attributed_party.map(|p| p.into_uuid()),
        encoded(terms::encode(&c.terms))?,
        super::encode_evidence(evidence)?
    ]))
}
fn fingerprint(
    write: &ShippingImportWrite,
    op: &ShippingImportOperation<'_>,
) -> Result<Vec<u8>, ShippingError> {
    let payload = match op {
        ShippingImportOperation::CreateProfile {
            creation_key,
            content: c,
            evidence,
        } => json!(["CREATE_PROFILE", creation_key, content(c, evidence)?]),
        ShippingImportOperation::ReplaceProfile(c) => json!([
            "REPLACE_PROFILE",
            c.profile_id.into_uuid(),
            c.expected_revision.into_inner(),
            content(&c.content, &c.evidence)?
        ]),
        ShippingImportOperation::ProfileLifecycle {
            command: c,
            restore,
        } => json!([
            "PROFILE_LIFECYCLE",
            c.profile_id.into_uuid(),
            c.expected_revision.into_inner(),
            restore
        ]),
        ShippingImportOperation::AssignDefault(c) => json!([
            "ASSIGN_DEFAULT",
            c.profile_id.into_uuid(),
            c.expected_revision.into_inner()
        ]),
        ShippingImportOperation::ClearDefault(c) => {
            json!(["CLEAR_DEFAULT", c.expected_revision.into_inner()])
        }
        ShippingImportOperation::Listing {
            key,
            expected_version,
            patch: p,
        } => {
            let p = match p {
                ImportListingPatch::Unchanged => json!("UNCHANGED"),
                ImportListingPatch::Clear => json!("CLEAR"),
                ImportListingPatch::Set {
                    integration,
                    selection,
                    rate_inputs,
                    shipping,
                    collection,
                } => json!([
                    encoded(codec::encode_integration(integration))?,
                    patch(
                        selection,
                        codec::encode_selection::<terms::StoredReferences>
                    )?,
                    patch(rate_inputs, codec::encode_inputs)?,
                    patch(shipping, codec::encode_shipping::<terms::StoredReferences>)?,
                    patch(
                        collection,
                        codec::encode_collection::<terms::StoredReferences>
                    )?
                ]),
            };
            json!([
                "LISTING",
                key.listing_source_id.into_uuid(),
                key.source_listing_id.as_ref(),
                expected_version,
                p
            ])
        }
    };
    let observation = write.observation.as_ref().map(|o| {
        json!([
            o.acquired_at.unix_timestamp_nanos().to_string(),
            o.fresh_until.unix_timestamp_nanos().to_string(),
            o.expires_at.unix_timestamp_nanos().to_string()
        ])
    });
    Ok(hash(
        serde_json::to_vec(&json!([
            1,
            write.connection_revision,
            write.sequence,
            write.acquired_at.unix_timestamp_nanos().to_string(),
            write.object_key,
            observation,
            payload
        ]))
        .map_err(invalid)?,
    ))
}
#[derive(FromRow)]
struct ConnectionRow {
    revision: i64,
    actor: String,
    integration_method: Option<String>,
    integration_domain: Option<String>,
    active: bool,
    connected_at: OffsetDateTime,
}
impl ConnectionRow {
    fn integration(&self) -> Result<DeliveryIntegration, ShippingError> {
        match (&self.integration_method, &self.integration_domain) {
            (None, None) => Ok(DeliveryIntegration::Source),
            (Some(method), Some(domain)) => codec::decode_integration(
                json!({"kind":"PROVIDER","method":method,"domain":domain}),
            )
            .map_err(super::corrupt_box),
            _ => Err(corrupt()),
        }
    }
    fn owner(&self) -> Vec<u8> {
        hash(self.actor.as_bytes())
    }
    fn producer(&self) -> Result<Vec<u8>, ShippingError> {
        Ok(hash(
            serde_json::to_vec(&json!([
                self.actor,
                encoded(codec::encode_integration(&self.integration()?))?
            ]))
            .map_err(invalid)?,
        ))
    }
    fn view(&self) -> Result<ShippingConnectionView, ShippingError> {
        Ok(ShippingConnectionView {
            revision: super::positive(self.revision)?,
            integration: self.integration()?,
            active: self.active,
            connected_at: self.connected_at,
        })
    }
}
#[derive(FromRow)]
struct OwnerRow {
    owner_hash: Option<Vec<u8>>,
    connection_revision: i64,
    sequence: i64,
    verified_at: Option<OffsetDateTime>,
}
fn check_owner(
    row: Option<OwnerRow>,
    existing: bool,
    producer: &[u8],
    write: &ShippingImportWrite,
) -> Result<(), ShippingError> {
    if let Some(row) = row {
        if row.owner_hash.as_deref() != Some(producer) {
            return Err(ShippingError::ImportOwnershipConflict);
        }
        if row.connection_revision == write.connection_revision as i64
            && row.sequence >= write.sequence as i64
        {
            return Err(ShippingError::ImportOutOfOrder);
        }
        if write.observation.as_ref().is_some_and(|o| {
            row.verified_at.is_some_and(|at| {
                // Match PostgreSQL timestamp precision so the same acquisition cannot
                // gain freshness by retaining nanoseconds discarded during persistence.
                o.acquired_at.unix_timestamp_nanos() / 1000 <= at.unix_timestamp_nanos() / 1000
            })
        }) {
            return Err(ShippingError::ImportOutOfOrder);
        }
    } else if existing {
        return Err(ShippingError::ImportOwnershipConflict);
    }
    Ok(())
}
fn components(op: &ShippingImportOperation<'_>) -> Vec<&'static str> {
    match op {
        ShippingImportOperation::Listing {
            patch: ImportListingPatch::Clear,
            ..
        } => vec!["SELECTION", "RATE_INPUTS", "SHIPPING", "COLLECTION"],
        ShippingImportOperation::Listing {
            patch:
                ImportListingPatch::Set {
                    selection,
                    rate_inputs,
                    shipping,
                    collection,
                    ..
                },
            ..
        } => [
            ("SELECTION", !matches!(selection, PatchField::Unchanged)),
            ("RATE_INPUTS", !matches!(rate_inputs, PatchField::Unchanged)),
            ("SHIPPING", !matches!(shipping, PatchField::Unchanged)),
            ("COLLECTION", !matches!(collection, PatchField::Unchanged)),
        ]
        .into_iter()
        .filter_map(|(k, v)| v.then_some(k))
        .collect(),
        _ => vec![],
    }
}
impl Repository<'_> {
    async fn current(
        &mut self,
        context: &OperationContext,
        source: ListingSourceId,
        revision: u64,
    ) -> Result<ConnectionRow, ShippingError> {
        let row=sqlx::query_as::<_,ConnectionRow>("SELECT revision,actor,integration_method,integration_domain,active,connected_at FROM shipping_import_connections WHERE listing_source_id=$1 FOR UPDATE")
            .bind(source.into_uuid()).fetch_optional(&mut *self.0).await.map_err(sql_error)?.ok_or(ShippingError::ImportConnectionStale)?;
        let eligible: bool = sqlx::query_scalar("SELECT shipping_import_connection_current($1,$2)")
            .bind(source.into_uuid())
            .bind(row.revision)
            .fetch_one(&mut *self.0)
            .await
            .map_err(sql_error)?;
        if row.revision <= 0
            || row.revision as u64 != revision
            || !eligible
            || row.actor != shipping_service::use_cases::import_actor(context)
        {
            return Err(ShippingError::ImportConnectionStale);
        }
        Ok(row)
    }
    async fn signal(
        &mut self,
        source: ListingSourceId,
        kind: &str,
        id: Uuid,
        observed: bool,
    ) -> Result<(), ShippingError> {
        sqlx::query("INSERT INTO shipping_validity_changes(listing_source_id,subject_kind,subject_id,revision,change_kind) SELECT $1,$2,$3,COALESCE(MAX(revision),0)+1,$4 FROM shipping_validity_changes WHERE subject_kind=$2 AND subject_id=$3")
            .bind(source.into_uuid()).bind(kind).bind(id).bind(if observed {"OBSERVED"}else{"OWNERSHIP_CHANGED"}).execute(&mut *self.0).await.map_err(sql_error)?;
        Ok(())
    }
}
#[async_trait::async_trait]
impl ShippingImportAccess for Repository<'_> {
    async fn manual_takeover(
        &mut self,
        source: ListingSourceId,
        operation: &ShippingImportOperation<'_>,
        subject: ShippingImportSubject,
    ) -> Result<(), ShippingError> {
        let (kind,id,changed)=match subject {
            ShippingImportSubject::Profile(id)=>("PROFILE",id.into_uuid(),sqlx::query("UPDATE shipping_profile_import_state SET owner_hash=NULL WHERE shipping_profile_id=$1 AND owner_hash IS NOT NULL").bind(id.into_uuid()).execute(&mut *self.0).await.map_err(sql_error)?.rows_affected()),
            ShippingImportSubject::SourceDefault=>("SOURCE_DEFAULT",source.into_uuid(),sqlx::query("INSERT INTO shipping_default_import_state(listing_source_id,connection_revision,sequence) VALUES($1,0,1) ON CONFLICT(listing_source_id) DO UPDATE SET owner_hash=NULL WHERE shipping_default_import_state.owner_hash IS NOT NULL").bind(source.into_uuid()).execute(&mut *self.0).await.map_err(sql_error)?.rows_affected()),
            ShippingImportSubject::Listing(id)=>{
                let parts=components(operation);
                let changed=sqlx::query("INSERT INTO listing_delivery_import_state(product_listing_id,component,connection_revision,sequence) SELECT $1,component,0,1 FROM unnest($2::text[]) AS component ON CONFLICT(product_listing_id,component) DO UPDATE SET owner_hash=NULL WHERE listing_delivery_import_state.owner_hash IS NOT NULL").bind(id.into_uuid()).bind(parts).execute(&mut *self.0).await.map_err(sql_error)?.rows_affected();
                ("LISTING",id.into_uuid(),changed)
            }
        };
        if changed > 0 {
            self.signal(source, kind, id, false).await?;
        }
        Ok(())
    }

    async fn prepare(
        &mut self,
        context: &OperationContext,
        source: ListingSourceId,
        write: &ShippingImportWrite,
        op: &ShippingImportOperation<'_>,
    ) -> Result<PreparedShippingImport, ShippingError> {
        write.validate()?;
        let connection = self
            .current(context, source, write.connection_revision)
            .await?;
        let integration = connection.integration()?;
        let digest = fingerprint(write, op)?;
        let receipt:Option<(Vec<u8>,String,Uuid,bool)>=sqlx::query_as("SELECT payload_hash,subject_kind,subject_id,replay_until>now() FROM shipping_import_request_fences WHERE listing_source_id=$1 AND connection_revision=$2 AND request_hash=$3")
            .bind(source.into_uuid()).bind(write.connection_revision as i64).bind(hash(&write.request_key)).fetch_optional(&mut *self.0).await.map_err(sql_error)?;
        if let Some((payload, kind, id, replayable)) = receipt {
            if payload != digest {
                return Err(ShippingError::ImportRequestConflict);
            }
            if !replayable {
                return Err(ShippingError::ImportOutOfOrder);
            }
            return Ok(PreparedShippingImport {
                replay: true,
                profile_id: if kind == "PROFILE" {
                    Some(ShippingProfileId::try_from(id).map_err(invalid)?)
                } else {
                    None
                },
                integration,
            });
        }
        if write.acquired_at < connection.connected_at
            || write.acquired_at > OffsetDateTime::now_utc()
        {
            return Err(ShippingError::ImportConnectionStale);
        }
        if let Some(observation) = &write.observation {
            observation.validate(OffsetDateTime::now_utc())?;
            if observation.acquired_at != write.acquired_at {
                return Err(ShippingError::ImportConnectionStale);
            }
        }
        if !matches!(op, ShippingImportOperation::CreateProfile { .. })
            && write.object_key.is_some()
        {
            return Err(ShippingError::InvalidInput);
        }
        let producer = connection.producer()?;
        let owner = connection.owner();
        match op {
            ShippingImportOperation::CreateProfile { .. } => {
                let key = write
                    .object_key
                    .as_ref()
                    .ok_or(ShippingError::InvalidInput)?;
                let existing:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM shipping_profile_import_state WHERE listing_source_id=$1 AND producer_hash=$2 AND object_hash=$3)").bind(source.into_uuid()).bind(&producer).bind(hash(key)).fetch_one(&mut *self.0).await.map_err(sql_error)?;
                if existing {
                    return Err(ShippingError::ImportIdentityExists);
                }
            }
            ShippingImportOperation::ReplaceProfile(c) => {
                let row=sqlx::query_as::<_,OwnerRow>("SELECT i.owner_hash,i.connection_revision,i.sequence,p.verified_at FROM shipping_profile_import_state i JOIN shipping_profiles p USING(shipping_profile_id) WHERE i.shipping_profile_id=$1 AND i.listing_source_id=$2 AND p.integration_method IS NOT DISTINCT FROM $3 AND p.integration_domain IS NOT DISTINCT FROM $4 FOR UPDATE OF i,p").bind(c.profile_id.into_uuid()).bind(source.into_uuid()).bind(&connection.integration_method).bind(&connection.integration_domain).fetch_optional(&mut *self.0).await.map_err(sql_error)?;
                check_owner(row, true, &owner, write)?;
            }
            ShippingImportOperation::ProfileLifecycle { command: c, .. } => {
                let row=sqlx::query_as::<_,OwnerRow>("SELECT i.owner_hash,i.connection_revision,i.sequence,p.verified_at FROM shipping_profile_import_state i JOIN shipping_profiles p USING(shipping_profile_id) WHERE i.shipping_profile_id=$1 AND i.listing_source_id=$2 FOR UPDATE OF i,p").bind(c.profile_id.into_uuid()).bind(source.into_uuid()).fetch_optional(&mut *self.0).await.map_err(sql_error)?;
                check_owner(row, true, &owner, write)?;
            }
            ShippingImportOperation::AssignDefault(_)
            | ShippingImportOperation::ClearDefault(_) => {
                let row=sqlx::query_as::<_,OwnerRow>("SELECT owner_hash,connection_revision,sequence,verified_at FROM shipping_default_import_state WHERE listing_source_id=$1 FOR UPDATE").bind(source.into_uuid()).fetch_optional(&mut *self.0).await.map_err(sql_error)?;
                let exists:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM shipping_source_defaults WHERE listing_source_id=$1)").bind(source.into_uuid()).fetch_one(&mut *self.0).await.map_err(sql_error)?;
                check_owner(row, exists, &owner, write)?;
                if let ShippingImportOperation::AssignDefault(c) = op {
                    let compatible:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM shipping_profiles p WHERE p.shipping_profile_id=$1 AND p.listing_source_id=$2 AND p.current_source_id=$2 AND p.lifecycle='ACTIVE' AND p.integration_method IS NOT DISTINCT FROM $3 AND p.integration_domain IS NOT DISTINCT FROM $4 AND (p.expires_at IS NULL OR p.expires_at>now()) AND NOT EXISTS(SELECT 1 FROM shipping_profile_import_state i WHERE i.shipping_profile_id=p.shipping_profile_id AND i.owner_hash IS NOT NULL AND NOT shipping_import_connection_current(p.listing_source_id,i.connection_revision)))")
                        .bind(c.profile_id.into_uuid()).bind(source.into_uuid()).bind(&connection.integration_method).bind(&connection.integration_domain).fetch_one(&mut *self.0).await.map_err(sql_error)?;
                    if !compatible {
                        return Err(ShippingError::ImportConnectionStale);
                    }
                }
            }
            ShippingImportOperation::Listing {
                key, patch: input, ..
            } => {
                let (id,value):(Uuid,Option<Value>)=sqlx::query_as("SELECT product_listing_id,delivery_configuration FROM product_listings WHERE listing_source_id=$1 AND source_listing_id=$2 FOR UPDATE").bind(source.into_uuid()).bind(key.source_listing_id.as_ref()).fetch_optional(&mut *self.0).await.map_err(sql_error)?.ok_or(ShippingError::ImportDependencyMissing)?;
                let delivery = value
                    .map(codec::decode::<super::terms::StoredReferences>)
                    .transpose()
                    .map_err(super::corrupt_box)?;
                if let ImportListingPatch::Set {
                    integration: requested,
                    ..
                } = input
                    && **requested != integration
                {
                    return Err(ShippingError::ImportConnectionStale);
                }
                let parts = components(op);
                if parts.is_empty() {
                    return Err(ShippingError::InvalidInput);
                }
                for part in parts {
                    let row=sqlx::query_as::<_,OwnerRow>("SELECT owner_hash,connection_revision,sequence,verified_at FROM listing_delivery_import_state WHERE product_listing_id=$1 AND component=$2 FOR UPDATE").bind(id).bind(part).fetch_optional(&mut *self.0).await.map_err(sql_error)?;
                    let existing = delivery.as_ref().is_some_and(|v| match part {
                        // Implicit SOURCE inheritance can accompany a manual override of
                        // another component. Explicit manual selections have their own fence.
                        "SELECTION" => v.selection().is_some_and(|s| {
                            s != &ProfileSelection::InheritSourceDefault
                                || v.integration() != &DeliveryIntegration::Source
                        }),
                        "RATE_INPUTS" => v.rate_inputs() != &ListingRateInputs::default(),
                        "SHIPPING" => v.overrides().shipping != ComponentOverride::Inherit,
                        _ => v.overrides().collection != ComponentOverride::Inherit,
                    });
                    let unobserved_claim =
                        row.is_none() && input.asserts_content() && write.observation.is_none();
                    check_owner(row, existing, &owner, write)?;
                    if unobserved_claim {
                        return Err(ShippingError::InvalidInput);
                    }
                }
            }
        }
        Ok(PreparedShippingImport {
            replay: false,
            profile_id: None,
            integration,
        })
    }
    async fn complete(
        &mut self,
        context: &OperationContext,
        source: ListingSourceId,
        write: &ShippingImportWrite,
        op: &ShippingImportOperation<'_>,
        subject: ShippingImportSubject,
    ) -> Result<(), ShippingError> {
        let connection = self
            .current(context, source, write.connection_revision)
            .await?;
        let producer = connection.producer()?;
        let owner = connection.owner();
        let observed = write.observation.is_some();
        let acquired = write.observation.as_ref().map(|o| o.acquired_at);
        let fresh = write.observation.as_ref().map(|o| o.fresh_until);
        let expiry = write.observation.as_ref().map(|o| o.expires_at);
        let (kind, id) = match subject {
            ShippingImportSubject::Profile(id) => {
                if let ShippingImportOperation::CreateProfile { .. } = op {
                    sqlx::query("INSERT INTO shipping_profile_import_state(shipping_profile_id,listing_source_id,producer_hash,object_hash,owner_hash,connection_revision,sequence) VALUES($1,$2,$3,$4,$5,$6,$7)")
                        .bind(id.into_uuid()).bind(source.into_uuid()).bind(&producer).bind(hash(write.object_key.as_ref().ok_or(ShippingError::InvalidInput)?)).bind(&owner).bind(write.connection_revision as i64).bind(write.sequence as i64).execute(&mut *self.0).await.map_err(sql_error)?;
                    sqlx::query("UPDATE shipping_profiles SET integration_method=$2,integration_domain=$3 WHERE shipping_profile_id=$1").bind(id.into_uuid()).bind(&connection.integration_method).bind(&connection.integration_domain).execute(&mut *self.0).await.map_err(sql_error)?;
                }
                sqlx::query("UPDATE shipping_profile_import_state SET owner_hash=$2,connection_revision=$3,sequence=$4,observation_revision=observation_revision+$5 WHERE shipping_profile_id=$1")
                    .bind(id.into_uuid()).bind(&owner).bind(write.connection_revision as i64).bind(write.sequence as i64).bind(i64::from(observed)).execute(&mut *self.0).await.map_err(sql_error)?;
                if observed {
                    sqlx::query("UPDATE shipping_profiles SET verified_at=$2,fresh_until=$3,expires_at=$4 WHERE shipping_profile_id=$1").bind(id.into_uuid()).bind(acquired).bind(fresh).bind(expiry).execute(&mut *self.0).await.map_err(sql_error)?;
                }
                ("PROFILE", id.into_uuid())
            }
            ShippingImportSubject::SourceDefault => {
                sqlx::query("INSERT INTO shipping_default_import_state(listing_source_id,owner_hash,connection_revision,sequence,observation_revision,verified_at,fresh_until,expires_at) VALUES($1,$2,$3,$4,$5,$6,$7,$8) ON CONFLICT(listing_source_id) DO UPDATE SET owner_hash=$2,connection_revision=$3,sequence=$4,observation_revision=shipping_default_import_state.observation_revision+$5,verified_at=COALESCE($6,shipping_default_import_state.verified_at),fresh_until=COALESCE($7,shipping_default_import_state.fresh_until),expires_at=COALESCE($8,shipping_default_import_state.expires_at)")
                    .bind(source.into_uuid()).bind(&owner).bind(write.connection_revision as i64).bind(write.sequence as i64).bind(i64::from(observed)).bind(acquired).bind(fresh).bind(expiry).execute(&mut *self.0).await.map_err(sql_error)?;
                ("SOURCE_DEFAULT", source.into_uuid())
            }
            ShippingImportSubject::Listing(id) => {
                for part in components(op) {
                    sqlx::query("INSERT INTO listing_delivery_import_state(product_listing_id,component,owner_hash,connection_revision,sequence,observation_revision,verified_at,fresh_until,expires_at) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9) ON CONFLICT(product_listing_id,component) DO UPDATE SET owner_hash=$3,connection_revision=$4,sequence=$5,observation_revision=listing_delivery_import_state.observation_revision+$6,verified_at=COALESCE($7,listing_delivery_import_state.verified_at),fresh_until=COALESCE($8,listing_delivery_import_state.fresh_until),expires_at=COALESCE($9,listing_delivery_import_state.expires_at)")
                        .bind(id.into_uuid()).bind(part).bind(&owner).bind(write.connection_revision as i64).bind(write.sequence as i64).bind(i64::from(observed)).bind(acquired).bind(fresh).bind(expiry).execute(&mut *self.0).await.map_err(sql_error)?;
                }
                sqlx::query("UPDATE product_listings SET delivery_fresh_until=NULL,delivery_expires_at=NULL WHERE product_listing_id=$1").bind(id.into_uuid()).execute(&mut *self.0).await.map_err(sql_error)?;
                ("LISTING", id.into_uuid())
            }
        };
        if observed {
            self.signal(source, kind, id, true).await?;
        }
        sqlx::query("INSERT INTO shipping_import_request_fences(listing_source_id,connection_revision,request_hash,payload_hash,subject_kind,subject_id) VALUES($1,$2,$3,$4,$5,$6)")
            .bind(source.into_uuid()).bind(write.connection_revision as i64).bind(hash(&write.request_key)).bind(fingerprint(write,op)?).bind(kind).bind(id).execute(&mut *self.0).await.map_err(sql_error)?;
        Ok(())
    }
    async fn connection(
        &mut self,
        source: ListingSourceId,
    ) -> Result<Option<ShippingConnectionView>, ShippingError> {
        sqlx::query_as::<_,ConnectionRow>("SELECT revision,actor,integration_method,integration_domain,active AND shipping_import_connection_current(listing_source_id,revision) AS active,connected_at FROM shipping_import_connections WHERE listing_source_id=$1")
            .bind(source.into_uuid()).fetch_optional(&mut *self.0).await.map_err(sql_error)?.map(|r|r.view()).transpose()
    }
    async fn set_connection(
        &mut self,
        context: &OperationContext,
        c: &ChangeShippingConnectionCommand,
    ) -> Result<ShippingConnectionView, ShippingError> {
        let current = self.connection(c.source_id).await?;
        if current.as_ref().map_or(0, |v| v.revision) != c.expected_revision {
            return Err(ShippingError::StaleRevision);
        }
        let (method, domain) = match &c.integration {
            DeliveryIntegration::Source => (None, None),
            DeliveryIntegration::Provider { method, domain } => {
                if !matches!(
                    method,
                    listing_source_core::ListingIngestionMethod::Shopify
                        | listing_source_core::ListingIngestionMethod::Woocommerce
                ) {
                    return Err(ShippingError::InvalidInput);
                }
                (Some(method.as_str()), Some(domain.as_str()))
            }
        };
        if c.active {
            let enabled: bool = if let Some(method) = method {
                sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM listing_source_ingestion_methods WHERE listing_source_id=$1 AND ingestion_method=$2)").bind(c.source_id.into_uuid()).bind(method).fetch_one(&mut *self.0).await.map_err(sql_error)?
            } else {
                sqlx::query_scalar("SELECT NOT EXISTS(SELECT 1 FROM listing_source_ingestion_methods WHERE listing_source_id=$1 AND ingestion_method IN ('SHOPIFY','WOOCOMMERCE'))").bind(c.source_id.into_uuid()).fetch_one(&mut *self.0).await.map_err(sql_error)?
            };
            if !enabled {
                return Err(ShippingError::ImportConnectionStale);
            }
            if method == Some("SHOPIFY") {
                let matches:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM listing_source_shopify_ingestion_configurations WHERE listing_source_id=$1 AND domain=$2)").bind(c.source_id.into_uuid()).bind(domain).fetch_one(&mut *self.0).await.map_err(sql_error)?;
                if !matches {
                    return Err(ShippingError::ImportConnectionStale);
                }
            } else if method == Some("WOOCOMMERCE") {
                sqlx::query("UPDATE listing_source_woocommerce_ingestion_configurations SET shipping_integration_domain=$2 WHERE listing_source_id=$1 AND shipping_integration_domain IS DISTINCT FROM $2")
                    .bind(c.source_id.into_uuid()).bind(domain).execute(&mut *self.0).await.map_err(sql_error)?;
            }
        }
        let user = match context.principal {
            application::operation_context::Principal::User(id) => Some(id.into_uuid()),
            _ => None,
        };
        let revision:i64=sqlx::query_scalar("SELECT COALESCE((SELECT revision FROM shipping_import_connections WHERE listing_source_id=$1),0)+1").bind(c.source_id.into_uuid()).fetch_one(&mut *self.0).await.map_err(sql_error)?;
        let row=sqlx::query_as::<_,ConnectionRow>("INSERT INTO shipping_import_connections(listing_source_id,revision,actor,user_id,integration_method,integration_domain,active,connected_at) VALUES($1,$2,$3,$4,$5,$6,$7,clock_timestamp()) ON CONFLICT(listing_source_id) DO UPDATE SET revision=$2,actor=$3,user_id=$4,integration_method=$5,integration_domain=$6,active=$7,connected_at=clock_timestamp() RETURNING revision,actor,integration_method,integration_domain,active,connected_at")
            .bind(c.source_id.into_uuid()).bind(revision).bind(shipping_service::use_cases::import_actor(context)).bind(user).bind(method).bind(domain).bind(c.active).fetch_one(&mut *self.0).await.map_err(sql_error)?;
        sqlx::query("INSERT INTO shipping_validity_changes(listing_source_id,subject_kind,subject_id,revision,change_kind) VALUES($1,'CONNECTION',$1,$2,'CONNECTION_CHANGED')").bind(c.source_id.into_uuid()).bind(revision).execute(&mut *self.0).await.map_err(sql_error)?;
        row.view()
    }
    async fn resolve_profile(
        &mut self,
        context: &OperationContext,
        source: ListingSourceId,
        revision: u64,
        key: &str,
    ) -> Result<ShippingProfileId, ShippingError> {
        let connection = self.current(context, source, revision).await?;
        let id:Uuid=sqlx::query_scalar("SELECT shipping_profile_id FROM shipping_profile_import_state WHERE listing_source_id=$1 AND producer_hash=$2 AND object_hash=$3")
            .bind(source.into_uuid()).bind(connection.producer()?).bind(hash(key)).fetch_optional(&mut *self.0).await.map_err(sql_error)?.ok_or(ShippingError::ProfileNotFound)?;
        ShippingProfileId::try_from(id).map_err(invalid)
    }
}

//! Durable shipping fan-out and compact read state. No provider I/O or listing rewrites.
use super::shipping_projection_codec::{decode, encode};
use application::error::{box_error, static_error};
use platform_postgres::SqlxTransaction;
use product_listing_service::shipping_projection::{
    EffectiveListingShipping, POLICY, ShippingClaim, ShippingPage, ShippingProjectionError,
    ShippingProjectionStore, ShippingProjectionStoreFactory, ShippingWorkId, ShippingWrite,
};
use shipping_service::assessment::{AssessmentDependencies, AssessmentReader};
use sqlx::{FromRow, PgConnection};
use uuid::Uuid;

#[derive(Clone, Copy, Default)]
pub struct SqlxShippingProjectionStoreFactory;
impl ShippingProjectionStoreFactory<SqlxTransaction> for SqlxShippingProjectionStoreFactory {
    fn in_transaction<'a>(
        &'a self,
        tx: &'a mut SqlxTransaction,
    ) -> impl ShippingProjectionStore + 'a {
        Store {
            connection: tx.connection(),
        }
    }
}
struct Store<'a> {
    connection: &'a mut PgConnection,
}
fn storage(e: impl std::error::Error + Send + Sync + 'static) -> ShippingProjectionError {
    ShippingProjectionError::Storage(box_error(e))
}
fn invalid() -> ShippingProjectionError {
    ShippingProjectionError::Storage(static_error("invalid persisted shipping projection work"))
}
#[derive(FromRow)]
struct WorkRow {
    work_id: Uuid,
    generation: i64,
    dependency_kind: String,
    dependency_id: String,
    after_listing: Option<Uuid>,
}
impl WorkRow {
    fn kind_invalid(&self) -> bool {
        if self
            .after_listing
            .is_some_and(|id| domain_primitives::object_id::validate_uuid(id).is_err())
        {
            return true;
        }
        if self.dependency_kind == "PROVIDER" {
            return self.dependency_id.is_empty() || self.dependency_id.len() > 255;
        }
        if !matches!(
            self.dependency_kind.as_str(),
            "SOURCE" | "PROFILE" | "LISTING" | "LOCATION" | "RESOLUTION"
        ) {
            return true;
        }
        Uuid::parse_str(&self.dependency_id)
            .ok()
            .filter(|id| id.to_string() == self.dependency_id)
            .and_then(|id| domain_primitives::object_id::validate_uuid(id).ok())
            .is_none()
    }
}
#[async_trait::async_trait]
impl ShippingProjectionStore for Store<'_> {
    async fn claim(
        &mut self,
        work: ShippingWorkId,
    ) -> Result<Option<ShippingClaim>, ShippingProjectionError> {
        let row=sqlx::query_as::<_,WorkRow>("SELECT * FROM listing_shipping_work WHERE pending AND (lease_until IS NULL OR lease_until<=clock_timestamp()) AND work_id=$1 ORDER BY pending_since,work_id LIMIT 1 FOR UPDATE SKIP LOCKED")
            .bind(work.into_uuid()).fetch_optional(&mut *self.connection).await.map_err(storage)?;
        let Some(row) = row else {
            let pending: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM listing_shipping_work WHERE work_id=$1 AND pending)",
            )
            .bind(work.into_uuid())
            .fetch_one(&mut *self.connection)
            .await
            .map_err(storage)?;
            if pending {
                return Err(ShippingProjectionError::Deferred);
            }

            return Ok(None);
        };
        if row.kind_invalid() {
            return Err(invalid());
        }
        if row.generation < 1 {
            return Err(invalid());
        }
        let age: Option<f64> = sqlx::query_scalar("SELECT extract(epoch FROM clock_timestamp()-min(pending_since))::float8 FROM listing_shipping_work WHERE pending")
            .fetch_one(&mut *self.connection).await.map_err(storage)?;
        tracing::info!(
            metric = "listing_shipping_backlog",
            oldest_pending_seconds = age.unwrap_or(0.0)
        );
        let token = Uuid::new_v4();
        sqlx::query("UPDATE listing_shipping_work SET lease_token=$2,lease_until=clock_timestamp()+interval '60 seconds' WHERE work_id=$1").bind(row.work_id).bind(token).execute(&mut *self.connection).await.map_err(storage)?;
        Ok(Some(ShippingClaim {
            work_id: row.work_id.try_into().map_err(storage)?,
            generation: row.generation,
            token,
            kind: row.dependency_kind,
            dependency_id: row.dependency_id,
            after_listing: row.after_listing,
        }))
    }
    async fn page(
        &mut self,
        claim: &ShippingClaim,
        limit: u16,
    ) -> Result<ShippingPage, ShippingProjectionError> {
        if !(1..=100).contains(&limit) {
            return Err(ShippingProjectionError::InvalidInput);
        }
        sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ")
            .execute(&mut *self.connection)
            .await
            .map_err(storage)?;
        let current:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM listing_shipping_work WHERE work_id=$1 AND generation=$2 AND lease_token=$3 AND lease_until>clock_timestamp())").bind(claim.work_id.into_uuid()).bind(claim.generation).bind(claim.token).fetch_one(&mut *self.connection).await.map_err(storage)?;
        if !current {
            return Err(ShippingProjectionError::Superseded);
        }
        let selection = match claim.kind.as_str() {
            "LISTING" => {
                "SELECT product_listing_id FROM product_listings WHERE product_listing_id=$1::uuid"
            }
            "SOURCE" => {
                "SELECT product_listing_id FROM product_listings WHERE listing_source_id=$1::uuid"
            }
            "PROFILE" | "LOCATION" | "RESOLUTION" | "PROVIDER" => {
                "SELECT product_listing_id FROM listing_shipping_dependencies WHERE dependency_kind=$4 AND dependency_id=$1"
            }
            _ => return Err(invalid()),
        };
        // Indexed keysets bound dependency traversals. A new/unprocessed assignment
        // has independent LISTING work; migration/rebuild has SOURCE work. Neither
        // can be lost behind this reverse-index cursor before its first projection.
        // All variants accept the same bindings; unused kind is referenced below.
        let query = format!(
            "WITH candidates AS ({selection}) SELECT product_listing_id FROM candidates WHERE ($2::uuid IS NULL OR product_listing_id>$2) AND length($4::text)>0 ORDER BY product_listing_id LIMIT $3"
        );
        let ids: Vec<Uuid> = sqlx::query_scalar(sqlx::AssertSqlSafe(query.as_str()))
            .bind(&claim.dependency_id)
            .bind(claim.after_listing)
            .bind(i64::from(limit))
            .bind(&claim.kind)
            .fetch_all(&mut *self.connection)
            .await
            .map_err(storage)?;
        // Match management's owner -> source -> profile -> location -> listing order.
        sqlx::query("SELECT party_id FROM parties WHERE party_id IN (SELECT operator_party_id FROM listing_sources s JOIN product_listings p USING(listing_source_id) WHERE p.product_listing_id=ANY($1)) ORDER BY party_id FOR SHARE").bind(&ids).fetch_all(&mut *self.connection).await.map_err(storage)?;
        sqlx::query("SELECT listing_source_id FROM listing_sources WHERE listing_source_id IN (SELECT listing_source_id FROM product_listings WHERE product_listing_id=ANY($1)) ORDER BY listing_source_id FOR SHARE").bind(&ids).fetch_all(&mut *self.connection).await.map_err(storage)?;
        sqlx::query("SELECT shipping_profile_id FROM shipping_profiles WHERE shipping_profile_id IN (SELECT id::uuid FROM listing_shipping_references($1) WHERE kind='PROFILE') ORDER BY shipping_profile_id FOR SHARE").bind(&ids).fetch_all(&mut *self.connection).await.map_err(storage)?;
        sqlx::query("SELECT party_location_id FROM party_locations WHERE party_location_id IN (SELECT id::uuid FROM listing_shipping_references($1) WHERE kind='LOCATION') ORDER BY party_location_id FOR SHARE").bind(&ids).fetch_all(&mut *self.connection).await.map_err(storage)?;
        sqlx::query("SELECT listing_source_id FROM shipping_import_authority WHERE listing_source_id IN (SELECT listing_source_id FROM product_listings WHERE product_listing_id=ANY($1)) ORDER BY listing_source_id FOR SHARE").bind(&ids).fetch_all(&mut *self.connection).await.map_err(storage)?;
        // Geography projection also locks listings before resolution state. Keep
        // that shared order so concurrent enrichment pages cannot deadlock.
        sqlx::query("SELECT product_listing_id FROM product_listings WHERE product_listing_id=ANY($1) ORDER BY product_listing_id FOR UPDATE").bind(&ids).fetch_all(&mut *self.connection).await.map_err(storage)?;
        // Resolution readers/writers share provider -> resolution -> binding order.
        sqlx::query("SELECT profile FROM geo_resolution_provider_controls WHERE profile IN (SELECT id FROM listing_shipping_references($1) WHERE kind='PROVIDER') ORDER BY profile FOR SHARE").bind(&ids).fetch_all(&mut *self.connection).await.map_err(storage)?;
        sqlx::query("SELECT resolution_id FROM geo_resolutions WHERE resolution_id IN (SELECT id::uuid FROM listing_shipping_references($1) WHERE kind='RESOLUTION') ORDER BY resolution_id FOR SHARE").bind(&ids).fetch_all(&mut *self.connection).await.map_err(storage)?;
        sqlx::query("SELECT subject_id FROM geo_resolution_bindings WHERE subject_kind='PARTY_LOCATION' AND subject_id IN (SELECT id::uuid FROM listing_shipping_references($1) WHERE kind='LOCATION') ORDER BY subject_kind,subject_id FOR SHARE").bind(&ids).fetch_all(&mut *self.connection).await.map_err(storage)?;
        let listing_ids = ids
            .iter()
            .copied()
            .map(|id| id.try_into().map_err(storage))
            .collect::<Result<Vec<_>, _>>()?;
        let dependencies = if listing_ids.is_empty() {
            AssessmentDependencies {
                evaluated_at: time::OffsetDateTime::now_utc(),
                listings: vec![],
                profiles: vec![],
                defaults: vec![],
                locations: vec![],
            }
        } else {
            shipping_postgres::assessment::SqlxAssessmentReaderFactory::projection_reader(
                self.connection,
            )
            .snapshot(&listing_ids, None)
            .await?
        };
        Ok(ShippingPage {
            dependencies,
            listing_ids,
            after_listing: ids.last().copied().or(claim.after_listing),
            complete: ids.len() < usize::from(limit),
        })
    }
    async fn apply(&mut self, writes: &[ShippingWrite]) -> Result<u32, ShippingProjectionError> {
        if writes.len() > 100 {
            return Err(ShippingProjectionError::InvalidInput);
        }
        let ids = writes
            .iter()
            .map(|w| w.listing_id.into_uuid())
            .collect::<Vec<_>>();
        // Rebuild reverse references in the same transaction as the summary/checkpoint.
        sqlx::query("DELETE FROM listing_shipping_dependencies WHERE product_listing_id=ANY($1)")
            .bind(&ids)
            .execute(&mut *self.connection)
            .await
            .map_err(storage)?;
        sqlx::query("INSERT INTO listing_shipping_dependencies SELECT product_listing_id,kind,id FROM listing_shipping_references($1)").bind(&ids).execute(&mut *self.connection).await.map_err(storage)?;
        let bases:Vec<(Uuid,serde_json::Value)>=sqlx::query_as(r#"
            SELECT p.product_listing_id,jsonb_build_object(
                'policy',$2::text,'evaluator',$3::text,'listingRevision',p.version,'lifecycle',p.lifecycle,
                'sourceId',p.listing_source_id,'sourceRevision',s.version,'operatorId',s.operator_party_id,'authorityRevision',a.revision,
                'listingEvidence',p.delivery_verified_at,
                'components',(SELECT jsonb_agg((to_jsonb(i)-'owner_hash')||jsonb_build_object('owned',i.owner_hash IS NOT NULL) ORDER BY component) FROM listing_delivery_import_state i WHERE i.product_listing_id=p.product_listing_id),
                'default',(SELECT jsonb_build_object('revision',d.revision,'profile',d.shipping_profile_id,'deleted',d.source_deleted,'observation',(to_jsonb(i)-'owner_hash')||jsonb_build_object('owned',i.owner_hash IS NOT NULL)) FROM shipping_source_defaults d LEFT JOIN shipping_default_import_state i USING(listing_source_id) WHERE d.listing_source_id=p.listing_source_id),
                'profiles',(SELECT jsonb_agg(jsonb_build_object('id',v.shipping_profile_id,'managementRevision',v.management_revision,'termsRevision',v.terms_revision,'lifecycle',v.lifecycle,'currentSource',v.current_source_id,'integration',v.integration_method,'verifiedAt',v.verified_at,'observationRevision',i.observation_revision,'owned',i.owner_hash IS NOT NULL,'sourceRevision',i.source_revision) ORDER BY v.shipping_profile_id) FROM listing_shipping_dependencies dep JOIN shipping_profiles v ON v.shipping_profile_id=CASE WHEN dep.dependency_kind='PROFILE' THEN dep.dependency_id::uuid END LEFT JOIN shipping_profile_import_state i USING(shipping_profile_id) WHERE dep.product_listing_id=p.product_listing_id AND dep.dependency_kind='PROFILE'),
                'locations',(SELECT jsonb_agg(jsonb_build_object('id',l.party_location_id,'revision',l.revision,'inputRevision',l.input_revision,'lifecycle',l.lifecycle,'bindingInputRevision',b.input_revision,'bindingGuard',b.guard,'selectedGeneration',b.selected_generation,'selectedCandidate',b.selected_candidate,'resolution',r.resolution_id,'generation',r.generation,'resolutionPolicy',r.policy_version,'validUntil',r.valid_until,'storagePermitted',c.storage_permitted) ORDER BY l.party_location_id) FROM listing_shipping_dependencies dep JOIN party_locations l ON l.party_location_id=CASE WHEN dep.dependency_kind='LOCATION' THEN dep.dependency_id::uuid END LEFT JOIN geo_resolution_bindings b ON b.subject_kind='PARTY_LOCATION' AND b.subject_id=l.party_location_id LEFT JOIN geo_resolutions r USING(resolution_id) LEFT JOIN geo_resolution_provider_controls c ON c.profile=r.profile WHERE dep.product_listing_id=p.product_listing_id AND dep.dependency_kind='LOCATION')
            ) FROM product_listings p JOIN listing_sources s USING(listing_source_id) LEFT JOIN shipping_import_authority a USING(listing_source_id) WHERE p.product_listing_id=ANY($1)
        "#).bind(&ids).bind(POLICY).bind(shipping_core::assessment::POLICY_VERSION).fetch_all(&mut *self.connection).await.map_err(storage)?;
        let mut changed = 0;
        for w in writes {
            let dependency = &bases
                .iter()
                .find(|(id, _)| *id == w.listing_id.into_uuid())
                .ok_or(ShippingProjectionError::Superseded)?
                .1;
            let summary = encode(&w.summary)?;
            let old:Option<(serde_json::Value,i64)>=sqlx::query_as("SELECT summary,projection_version FROM product_listing_shipping WHERE product_listing_id=$1").bind(w.listing_id.into_uuid()).fetch_optional(&mut *self.connection).await.map_err(storage)?;
            let differs = match &old {
                Some((value, _)) => decode(value.clone())? != w.summary,
                None => w.summary != EffectiveListingShipping::default(),
            };
            let mut version: i64 = sqlx::query_scalar(
                "SELECT projection_version FROM product_listings WHERE product_listing_id=$1",
            )
            .bind(w.listing_id.into_uuid())
            .fetch_one(&mut *self.connection)
            .await
            .map_err(storage)?;
            if old
                .as_ref()
                .is_some_and(|(_, old_version)| *old_version > version)
            {
                return Err(invalid());
            }
            if differs {
                let event = domain_primitives::event_id::EventId::new();
                sqlx::query("INSERT INTO product_listing_events(event_id,product_listing_id,event_type,event_group,event_type_schema_version,payload,event_time) VALUES($1,$2,'ENRICHMENT_SHIPPING_UPDATED','ENRICHMENT',1,'{}',now())").bind(event.into_uuid()).bind(w.listing_id.into_uuid()).execute(&mut *self.connection).await.map_err(storage)?;
                version=sqlx::query_scalar("UPDATE product_listings SET current_event_id=$2,projection_version=projection_version+1,updated=aura_advance_updated(updated) WHERE product_listing_id=$1 RETURNING projection_version").bind(w.listing_id.into_uuid()).bind(event.into_uuid()).fetch_one(&mut *self.connection).await.map_err(storage)?;
                changed += 1;
            } else if let Some((_, old_version)) = old {
                version = old_version;
            }
            sqlx::query("INSERT INTO product_listing_shipping(product_listing_id,dependency,summary,projection_version) VALUES($1,$2,$3,$4) ON CONFLICT(product_listing_id) DO UPDATE SET dependency=EXCLUDED.dependency,summary=EXCLUDED.summary,projection_version=EXCLUDED.projection_version").bind(w.listing_id.into_uuid()).bind(dependency).bind(summary).bind(version).execute(&mut *self.connection).await.map_err(storage)?;
        }
        Ok(changed)
    }
    async fn checkpoint(
        &mut self,
        claim: &ShippingClaim,
        page: &ShippingPage,
    ) -> Result<(), ShippingProjectionError> {
        let updated=sqlx::query("UPDATE listing_shipping_work SET after_listing=$4,pending=$5,generation=generation+1,lease_token=NULL,lease_until=NULL WHERE work_id=$1 AND generation=$2 AND lease_token=$3 AND lease_until>clock_timestamp()")
            .bind(claim.work_id.into_uuid()).bind(claim.generation).bind(claim.token).bind(page.after_listing).bind(!page.complete).execute(&mut *self.connection).await.map_err(storage)?;
        if updated.rows_affected() != 1 {
            return Err(ShippingProjectionError::Superseded);
        }
        if !page.complete {
            sqlx::query("INSERT INTO listing_shipping_wakeups(work_id,generation) VALUES($1,$2)")
                .bind(claim.work_id.into_uuid())
                .bind(claim.generation + 1)
                .execute(&mut *self.connection)
                .await
                .map_err(storage)?;
        }
        Ok(())
    }
}

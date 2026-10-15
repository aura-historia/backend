use application::error::{box_error, static_error};
use platform_postgres::SqlxTransaction;
use serde_json::json;
use shipping_core::discovery::{EffectiveShipping, POLICY};
use shipping_service::{
    assessment::{AssessmentDependencies, AssessmentReader},
    projection::*,
};
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
    ShippingProjectionError::Storage(static_error("invalid effective shipping state"))
}
#[derive(FromRow)]
struct WorkRow {
    work_id: Uuid,
    generation: i64,
    dependency_kind: String,
    dependency_id: String,
    after_listing: Option<Uuid>,
}
fn evidence(e: shipping_service::assessment::AssertionEvidence) -> serde_json::Value {
    json!({"verifiedAt":e.verified_at.unix_timestamp_nanos().to_string(),"freshUntil":e.fresh_until.map(|t|t.unix_timestamp_nanos().to_string()),"expiresAt":e.expires_at.map(|t|t.unix_timestamp_nanos().to_string())})
}
#[async_trait::async_trait]
impl ShippingProjectionStore for Store<'_> {
    async fn maintain(&mut self, limit: u16) -> Result<(), ShippingProjectionError> {
        // Advance each deadline once. The listing job retains custody through retries.
        let ids:Vec<Uuid> = sqlx::query_scalar("SELECT product_listing_id FROM product_listing_shipping WHERE valid_until<=clock_timestamp() ORDER BY valid_until,product_listing_id LIMIT $1 FOR UPDATE SKIP LOCKED")
            .bind(i64::from(limit)).fetch_all(&mut *self.connection).await.map_err(storage)?;
        for id in ids {
            sqlx::query("SELECT enqueue_listing_shipping('LISTING',$1)")
                .bind(id.to_string())
                .execute(&mut *self.connection)
                .await
                .map_err(storage)?;
            sqlx::query(
                "UPDATE product_listing_shipping SET valid_until=NULL WHERE product_listing_id=$1",
            )
            .bind(id)
            .execute(&mut *self.connection)
            .await
            .map_err(storage)?;
        }
        // A bounded fresh wake-up recovers every runnable pending page, independently
        // of how much of the old transport retention window remains.
        sqlx::query(r#"WITH runnable AS (
            SELECT work_id FROM listing_shipping_work WHERE pending AND (lease_until IS NULL OR lease_until<=clock_timestamp())
            ORDER BY pending_since,work_id LIMIT $1 FOR UPDATE SKIP LOCKED
        ), advanced AS (
            UPDATE listing_shipping_work w SET generation=generation+1,lease_token=NULL,lease_until=NULL
            FROM runnable r WHERE w.work_id=r.work_id RETURNING w.work_id,w.generation
        ) INSERT INTO listing_shipping_wakeups SELECT * FROM advanced"#)
            .bind(i64::from(limit)).execute(&mut *self.connection).await.map_err(storage)?;
        Ok(())
    }
    async fn claim(
        &mut self,
        work: Option<ShippingWorkId>,
    ) -> Result<Option<ShippingClaim>, ShippingProjectionError> {
        let row=sqlx::query_as::<_,WorkRow>("SELECT * FROM listing_shipping_work WHERE pending AND (lease_until IS NULL OR lease_until<=clock_timestamp()) AND ($1::uuid IS NULL OR work_id=$1) ORDER BY pending_since,work_id LIMIT 1 FOR UPDATE SKIP LOCKED")
            .bind(work.map(|w|w.into_uuid())).fetch_optional(&mut *self.connection).await.map_err(storage)?;
        let Some(r) = row else {
            if let Some(work) = work {
                let pending:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM listing_shipping_work WHERE work_id=$1 AND pending)").bind(work.into_uuid()).fetch_one(&mut *self.connection).await.map_err(storage)?;
                if pending {
                    return Err(ShippingProjectionError::Deferred);
                }
            }
            return Ok(None);
        };
        let token = Uuid::new_v4();
        sqlx::query("UPDATE listing_shipping_work SET lease_token=$2,lease_until=clock_timestamp()+interval '60 seconds' WHERE work_id=$1").bind(r.work_id).bind(token).execute(&mut *self.connection).await.map_err(storage)?;
        let age:Option<f64>=sqlx::query_scalar("SELECT EXTRACT(epoch FROM clock_timestamp()-min(pending_since))::float8 FROM listing_shipping_work WHERE pending").fetch_one(&mut *self.connection).await.map_err(storage)?;
        tracing::info!(
            metric = "listing_shipping_backlog",
            oldest_pending_seconds = age.unwrap_or(0.0)
        );
        Ok(Some(ShippingClaim {
            work_id: r.work_id.try_into().map_err(storage)?,
            generation: r.generation,
            token,
            kind: r.dependency_kind,
            dependency_id: r.dependency_id,
            after_listing: r.after_listing,
        }))
    }
    async fn page(
        &mut self,
        claim: &ShippingClaim,
        limit: u16,
    ) -> Result<ShippingPage, ShippingProjectionError> {
        // Profile ownership/default/source and retained reverse references provide
        // fanout without copying policy into listings. New assignments have their own jobs.
        let selection = match claim.kind.as_str() {
            "LISTING" => {
                "SELECT product_listing_id FROM product_listings WHERE product_listing_id=$1::uuid"
            }
            "SOURCE" => {
                "SELECT product_listing_id FROM product_listings WHERE listing_source_id=$1::uuid"
            }
            "PROFILE" => {
                "SELECT product_listing_id FROM product_listing_shipping_dependencies WHERE dependency_kind='PROFILE' AND dependency_id=$1::uuid
                UNION SELECT product_listing_id FROM product_listings WHERE delivery_configuration #>> '{selection,kind}'='EXPLICIT' AND delivery_configuration #>> '{selection,profile_id}'=$1::text
                UNION SELECT p.product_listing_id FROM shipping_source_defaults d JOIN product_listings p USING(listing_source_id) WHERE d.shipping_profile_id=$1::uuid AND (p.delivery_configuration IS NULL OR p.delivery_configuration #>> '{selection,kind}'='INHERIT_SOURCE_DEFAULT')"
            }
            "LOCATION" => {
                "SELECT product_listing_id FROM product_listing_shipping_dependencies WHERE dependency_kind='LOCATION' AND dependency_id=$1::uuid"
            }
            "RESOLUTION" => {
                "SELECT product_listing_id FROM product_listing_shipping_dependencies WHERE dependency_kind='RESOLUTION' AND dependency_id=$1::uuid"
            }
            "PROVIDER" => {
                "SELECT d.product_listing_id FROM product_listing_shipping_dependencies d JOIN geo_resolutions r ON r.resolution_id=d.dependency_id WHERE d.dependency_kind='RESOLUTION' AND r.profile=$1"
            }
            _ => return Err(invalid()),
        };
        let query = format!(
            "WITH selected AS ({selection}) SELECT DISTINCT p.product_listing_id,p.listing_source_id FROM selected s JOIN product_listings p USING(product_listing_id) WHERE ($2::uuid IS NULL OR p.product_listing_id>$2) ORDER BY p.product_listing_id LIMIT $3"
        );
        let ids: Vec<(Uuid, Uuid)> = sqlx::query_as(sqlx::AssertSqlSafe(query.as_str()))
            .bind(&claim.dependency_id)
            .bind(claim.after_listing)
            .bind(i64::from(limit))
            .fetch_all(&mut *self.connection)
            .await
            .map_err(storage)?;
        let listing_ids = ids.iter().map(|i| i.0).collect::<Vec<_>>();
        let sources = ids.iter().map(|i| i.1).collect::<Vec<_>>();
        // Same owner -> source order as shipping/location management. Listing locks
        // serialize all summary writers before dependency reads, including stale jobs.
        sqlx::query("SELECT party_id FROM parties WHERE party_id IN (SELECT operator_party_id FROM listing_sources WHERE listing_source_id=ANY($1)) ORDER BY party_id FOR SHARE").bind(&sources).fetch_all(&mut *self.connection).await.map_err(storage)?;
        sqlx::query("SELECT listing_source_id FROM listing_sources WHERE listing_source_id=ANY($1) ORDER BY listing_source_id FOR SHARE").bind(&sources).fetch_all(&mut *self.connection).await.map_err(storage)?;
        let locked:Vec<(Uuid,Uuid)>=sqlx::query_as("SELECT product_listing_id,listing_source_id FROM product_listings WHERE product_listing_id=ANY($1) ORDER BY product_listing_id FOR UPDATE").bind(&listing_ids).fetch_all(&mut *self.connection).await.map_err(storage)?;
        if locked != ids {
            return Err(ShippingProjectionError::Superseded);
        }
        let typed_ids = listing_ids
            .into_iter()
            .map(|id| id.try_into().map_err(storage))
            .collect::<Result<Vec<_>, _>>()?;
        let dependencies = if typed_ids.is_empty() {
            AssessmentDependencies {
                evaluated_at: time::OffsetDateTime::now_utc(),
                listings: vec![],
                defaults: vec![],
                profiles: vec![],
                locations: vec![],
            }
        } else {
            super::assessment::SqlxAssessmentReaderFactory::projection_reader(self.connection)
                .snapshot(&typed_ids, None)
                .await?
        };
        Ok(ShippingPage {
            dependencies,
            after_listing: ids.last().map(|i| i.0),
            complete: ids.len() < usize::from(limit),
        })
    }
    async fn apply(&mut self, writes: &[ShippingWrite]) -> Result<u32, ShippingProjectionError> {
        let mut changed = 0;
        for w in writes {
            let dependency = json!({"policy":POLICY,"evaluator":shipping_core::assessment::EVALUATOR_POLICY,"resolutionPolicy":geo_service::resolution::POLICY_VERSION,
                "sourceId":w.source_id.into_uuid(),"sourceRevision":w.source_revision,"listingRevision":w.listing_revision,"integrationCurrent":w.integration_current,
                "defaultRevision":w.default_revision,"profileReference":w.profile_reference.map(|p|p.into_uuid()),
                "profile":w.profile.as_ref().map(|p|json!({"id":p.id.into_uuid(),"termsRevision":p.terms_revision,"managementRevision":p.management_revision,"lifecycle":p.lifecycle.as_str(),"evidence":evidence(p.evidence)})),
                "evidence":w.evidence.map(evidence),"locationReferences":w.location_references.iter().map(|id|id.into_uuid()).collect::<Vec<_>>(),
                "locations":w.locations.iter().map(|l|json!({"id":l.id.into_uuid(),"revision":l.revision,"inputRevision":l.input_revision,"active":l.active,"resolutionId":l.resolution_id,"resolutionGeneration":l.resolution_generation,"validUntil":l.valid_until.map(|t|t.unix_timestamp_nanos().to_string())})).collect::<Vec<_>>()});
            let summary = shipping_json::discovery::encode(&w.summary)
                .map_err(ShippingProjectionError::Storage)?;
            let old:Option<(serde_json::Value,i64)>=sqlx::query_as("SELECT summary,shipping_version FROM product_listing_shipping WHERE product_listing_id=$1").bind(w.listing_id.into_uuid()).fetch_optional(&mut *self.connection).await.map_err(storage)?;
            let previous = old
                .as_ref()
                .map(|o| shipping_json::discovery::decode(o.0.clone()))
                .transpose()
                .map_err(ShippingProjectionError::Storage)?
                .unwrap_or_else(EffectiveShipping::default);
            let differs = previous != w.summary;
            let mut version:i64=sqlx::query_scalar("SELECT projection_version FROM product_listings WHERE product_listing_id=$1 AND version=$2 AND lifecycle='ACTIVE'").bind(w.listing_id.into_uuid()).bind(i64::try_from(w.listing_revision).map_err(storage)?).fetch_optional(&mut *self.connection).await.map_err(storage)?.ok_or(ShippingProjectionError::Superseded)?;
            if differs {
                let event = domain_primitives::event_id::EventId::new();
                sqlx::query("INSERT INTO product_listing_events(event_id,product_listing_id,event_type,event_group,event_type_schema_version,payload,event_time) VALUES($1,$2,'ENRICHMENT_SHIPPING_UPDATED','ENRICHMENT',1,'{}',now())").bind(event.into_uuid()).bind(w.listing_id.into_uuid()).execute(&mut *self.connection).await.map_err(storage)?;
                version=sqlx::query_scalar("UPDATE product_listings SET current_event_id=$2,projection_version=projection_version+1 WHERE product_listing_id=$1 RETURNING projection_version").bind(w.listing_id.into_uuid()).bind(event.into_uuid()).fetch_one(&mut *self.connection).await.map_err(storage)?;
                changed += 1;
            } else if let Some(old) = old {
                version = old.1;
            }
            sqlx::query("INSERT INTO product_listing_shipping(product_listing_id,dependency,summary,valid_until,shipping_version)VALUES($1,$2,$3,$4,$5) ON CONFLICT(product_listing_id) DO UPDATE SET dependency=EXCLUDED.dependency,summary=EXCLUDED.summary,valid_until=EXCLUDED.valid_until,shipping_version=EXCLUDED.shipping_version")
                .bind(w.listing_id.into_uuid()).bind(dependency).bind(summary).bind(w.valid_until).bind(version).execute(&mut *self.connection).await.map_err(storage)?;
            sqlx::query(
                "DELETE FROM product_listing_shipping_dependencies WHERE product_listing_id=$1",
            )
            .bind(w.listing_id.into_uuid())
            .execute(&mut *self.connection)
            .await
            .map_err(storage)?;
            let mut refs = vec![("SOURCE", w.source_id.into_uuid())];
            if let Some(p) = w.profile_reference {
                refs.push(("PROFILE", p.into_uuid()));
            }
            for id in &w.location_references {
                refs.push(("LOCATION", id.into_uuid()));
            }
            for l in &w.locations {
                refs.push(("LOCATION", l.id.into_uuid()));
                if let Some(r) = l.resolution_id {
                    refs.push(("RESOLUTION", r));
                }
            }
            refs.sort_unstable();
            refs.dedup();
            let (kinds, ids): (Vec<_>, Vec<_>) = refs.into_iter().unzip();
            sqlx::query("INSERT INTO product_listing_shipping_dependencies SELECT $1,kind,id FROM unnest($2::text[],$3::uuid[]) AS ref(kind,id)")
                .bind(w.listing_id.into_uuid()).bind(kinds).bind(ids).execute(&mut *self.connection).await.map_err(storage)?;
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
            sqlx::query("INSERT INTO listing_shipping_wakeups VALUES($1,$2)")
                .bind(claim.work_id.into_uuid())
                .bind(claim.generation + 1)
                .execute(&mut *self.connection)
                .await
                .map_err(storage)?;
        }
        Ok(())
    }
}

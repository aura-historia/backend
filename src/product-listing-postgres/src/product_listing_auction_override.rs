use application::error::box_error;
use domain_primitives::event_id::EventId;
use product_listing_core::product_listing_id::ProductListingId;
use product_listing_service::ports::{
    ProductListingAuctionOverride, ProductListingAuctionOverrideAudit,
    ProductListingAuctionOverrideError, ProductListingAuctionOverrideRepository,
    ProductListingAuctionOverrideRepositoryFactory, ProductListingAuctionPolicyVersion,
    ProductListingRawAuctionCapture, ProductListingRawAuctionContextAdmission,
};
use sqlx::PgConnection;
use time::OffsetDateTime;

#[derive(Debug, Clone, Copy, Default)]
pub struct SqlxProductListingAuctionOverrideRepositoryFactory;

struct SqlxProductListingAuctionOverrideRepository<'tx> {
    connection: &'tx mut PgConnection,
}

#[derive(sqlx::FromRow)]
struct OverrideRow {
    policy_version: i64,
    active: bool,
    release_capture_generation_fence: Option<i64>,
}

#[derive(sqlx::FromRow)]
struct RawAuctionAdmissionRow {
    capture_generation: i64,
    policy_version: Option<i64>,
    active: Option<bool>,
    release_capture_generation_fence: Option<i64>,
    floor_revision: Option<i64>,
    floor_generation: Option<i64>,
    floor_revision_generation: Option<i64>,
}

impl SqlxProductListingAuctionOverrideRepositoryFactory {
    pub fn new() -> Self {
        Self
    }
}

impl ProductListingAuctionOverrideRepositoryFactory<platform_postgres::SqlxTransaction>
    for SqlxProductListingAuctionOverrideRepositoryFactory
{
    fn in_transaction<'tx>(
        &'tx self,
        tx: &'tx mut platform_postgres::SqlxTransaction,
    ) -> impl ProductListingAuctionOverrideRepository + 'tx {
        SqlxProductListingAuctionOverrideRepository {
            connection: tx.connection(),
        }
    }
}

#[async_trait::async_trait]
impl ProductListingAuctionOverrideRepository for SqlxProductListingAuctionOverrideRepository<'_> {
    async fn lock(
        &mut self,
        product_listing_id: ProductListingId,
    ) -> Result<(), ProductListingAuctionOverrideError> {
        sqlx::query_scalar::<_, ()>("SELECT pg_advisory_xact_lock(hashtextextended($1::text, 0))")
            .bind(product_listing_id.as_uuid())
            .fetch_one(&mut *self.connection)
            .await
            .map_err(persistence)
    }

    async fn find(
        &mut self,
        product_listing_id: ProductListingId,
    ) -> Result<Option<ProductListingAuctionOverride>, ProductListingAuctionOverrideError> {
        sqlx::query_as::<_, OverrideRow>(
            "SELECT policy_version, active, release_capture_generation_fence \
             FROM product_listing_auction_overrides WHERE product_listing_id = $1",
        )
        .bind(product_listing_id.as_uuid())
        .fetch_optional(&mut *self.connection)
        .await
        .map_err(persistence)?
        .map(override_from_row)
        .transpose()
    }

    async fn admit_raw_auction_context(
        &mut self,
        product_listing_id: ProductListingId,
        raw_capture: ProductListingRawAuctionCapture,
    ) -> Result<ProductListingRawAuctionContextAdmission, ProductListingAuctionOverrideError> {
        let revision = raw_capture_revision_to_i64(raw_capture.revision)?;
        let capture_generation = raw_capture_generation_to_i64(raw_capture.capture_generation)?;
        let row = sqlx::query_as::<_, RawAuctionAdmissionRow>(
            r#"
            SELECT capture.generation AS capture_generation,
                   policy.policy_version,
                   policy.active,
                   policy.release_capture_generation_fence,
                   floor.last_capture_revision AS floor_revision,
                   floor.last_capture_generation AS floor_generation,
                   floor_revision.generation AS floor_revision_generation
            FROM product_listing_raw_revisions AS capture
            LEFT JOIN product_listing_auction_overrides AS policy
              ON policy.product_listing_id = $1
            LEFT JOIN product_listing_auction_override_floors AS floor
              ON floor.product_listing_id = policy.product_listing_id
             AND floor.product_listing_raw_stream_id = capture.product_listing_raw_stream_id
            LEFT JOIN product_listing_raw_revisions AS floor_revision
              ON floor_revision.product_listing_raw_stream_id = floor.product_listing_raw_stream_id
             AND floor_revision.revision = floor.last_capture_revision
            WHERE capture.product_listing_raw_stream_id = $2
              AND capture.revision = $3
            "#,
        )
        .bind(product_listing_id.as_uuid())
        .bind(raw_capture.product_listing_raw_stream_id.as_uuid())
        .bind(revision)
        .fetch_optional(&mut *self.connection)
        .await
        .map_err(persistence)?
        .ok_or_else(|| invalid("raw auction capture revision is missing"))?;
        if row.capture_generation != capture_generation {
            return Err(invalid(
                "raw auction capture generation does not match its revision",
            ));
        }
        let Some(policy_version) = row.policy_version else {
            return Ok(ProductListingRawAuctionContextAdmission::Admit);
        };
        if u64::try_from(policy_version)
            .ok()
            .filter(|value| *value >= 1)
            .is_none()
        {
            return Err(invalid("auction override policy version is invalid"));
        }
        let active = row
            .active
            .ok_or_else(|| invalid("auction override policy active state is missing"))?;
        let floor = match (
            row.floor_revision,
            row.floor_generation,
            row.floor_revision_generation,
        ) {
            (None, None, None) => None,
            (Some(revision), Some(generation), Some(actual_generation)) => {
                let revision = raw_capture_revision_from_i64(revision)?;
                let generation = raw_capture_generation_from_i64(generation)?;
                let actual_generation = raw_capture_generation_from_i64(actual_generation)?;
                if generation != actual_generation {
                    return Err(invalid(
                        "auction override floor generation does not match revision",
                    ));
                }
                Some((revision, generation))
            }
            _ => return Err(invalid("auction override floor is incomplete")),
        };
        if active {
            return Ok(ProductListingRawAuctionContextAdmission::Preserve);
        }
        let fence = row
            .release_capture_generation_fence
            .ok_or_else(|| invalid("inactive auction override has no capture fence"))
            .and_then(raw_capture_generation_from_i64)?;
        if raw_capture.capture_generation <= fence
            || floor.is_some_and(|(floor_revision, _)| raw_capture.revision <= floor_revision)
        {
            Ok(ProductListingRawAuctionContextAdmission::Preserve)
        } else {
            Ok(ProductListingRawAuctionContextAdmission::Admit)
        }
    }

    async fn activate(
        &mut self,
        audit: &ProductListingAuctionOverrideAudit,
        expected_version: ProductListingAuctionPolicyVersion,
    ) -> Result<ProductListingAuctionOverride, ProductListingAuctionOverrideError> {
        let expected = version_to_i64(expected_version)?;
        sqlx::query(
            r#"
            INSERT INTO product_listing_auction_corrections (
                audit_id, product_listing_id, actor_label, reason, previous_auction_id,
                current_auction_id, recorded_at
            ) VALUES ($1, $2, $3, $4, $5, $6, $7)
            "#,
        )
        .bind(audit.audit_id.as_uuid())
        .bind(audit.product_listing_id.as_uuid())
        .bind(&audit.actor_label)
        .bind(&audit.reason)
        .bind(audit.previous_auction_id.map(|value| *value.as_uuid()))
        .bind(audit.current_auction_id.map(|value| *value.as_uuid()))
        .bind(audit.recorded_at)
        .execute(&mut *self.connection)
        .await
        .map_err(persistence)?;

        let row = if expected == 0 {
            sqlx::query_as::<_, OverrideRow>(
                r#"
                INSERT INTO product_listing_auction_overrides (
                    product_listing_id, policy_version, active, correction_audit_id, updated
                ) VALUES ($1, 1, TRUE, $2, now())
                ON CONFLICT (product_listing_id) DO NOTHING
                RETURNING policy_version, active, release_capture_generation_fence
                "#,
            )
            .bind(audit.product_listing_id.as_uuid())
            .bind(audit.audit_id.as_uuid())
            .fetch_optional(&mut *self.connection)
            .await
            .map_err(persistence)?
        } else {
            sqlx::query_as::<_, OverrideRow>(
                r#"
                UPDATE product_listing_auction_overrides
                SET policy_version = policy_version + 1,
                    active = TRUE,
                    correction_audit_id = $1,
                    updated = now()
                WHERE product_listing_id = $2
                  AND policy_version = $3
                RETURNING policy_version, active, release_capture_generation_fence
                "#,
            )
            .bind(audit.audit_id.as_uuid())
            .bind(audit.product_listing_id.as_uuid())
            .bind(expected)
            .fetch_optional(&mut *self.connection)
            .await
            .map_err(persistence)?
        };
        row.map(override_from_row)
            .transpose()?
            .ok_or(ProductListingAuctionOverrideError::ConcurrencyConflict)
    }

    async fn release(
        &mut self,
        product_listing_id: ProductListingId,
        expected_version: ProductListingAuctionPolicyVersion,
        audit_id: EventId,
        actor_label: String,
        recorded_at: OffsetDateTime,
    ) -> Result<ProductListingAuctionOverride, ProductListingAuctionOverrideError> {
        let expected = version_to_i64(expected_version)?;
        // Sequence allocation is the release linearization point. It does not lock or serialize
        // raw streams, and is intentionally not rolled back with this transaction.
        let generation = sqlx::query_scalar::<_, i64>(
            "SELECT nextval('product_listing_raw_capture_generation_seq'::regclass)",
        )
        .fetch_one(&mut *self.connection)
        .await
        .map_err(persistence)?;
        if generation < 1 {
            return Err(invalid("raw capture generation fence is invalid"));
        }

        let row = sqlx::query_as::<_, OverrideRow>(
            r#"
            UPDATE product_listing_auction_overrides
            SET policy_version = policy_version + 1,
                active = FALSE,
                release_capture_generation_fence = $1,
                released_audit_id = $2,
                updated = now()
            WHERE product_listing_id = $3
              AND policy_version = $4
              AND active = TRUE
            RETURNING policy_version, active, release_capture_generation_fence
            "#,
        )
        .bind(generation)
        .bind(audit_id.as_uuid())
        .bind(product_listing_id.as_uuid())
        .bind(expected)
        .fetch_optional(&mut *self.connection)
        .await
        .map_err(persistence)?
        .ok_or(ProductListingAuctionOverrideError::ConcurrencyConflict)?;

        sqlx::query(
            r#"
            INSERT INTO product_listing_auction_override_releases (
                audit_id, product_listing_id, actor_label, recorded_at, capture_generation_fence
            ) VALUES ($1, $2, $3, $4, $5)
            "#,
        )
        .bind(audit_id.as_uuid())
        .bind(product_listing_id.as_uuid())
        .bind(actor_label)
        .bind(recorded_at)
        .bind(generation)
        .execute(&mut *self.connection)
        .await
        .map_err(persistence)?;

        // Release never locks raw heads or streams. The global fence covers captures allocated
        // before this point but not visible in this transaction's linked-head snapshot.
        sqlx::query(
            r#"
            INSERT INTO product_listing_auction_override_floors (
                product_listing_id, product_listing_raw_stream_id, last_capture_revision,
                last_capture_generation
            )
            SELECT $1, head.product_listing_raw_stream_id, revision.revision, revision.generation
            FROM product_listing_raw_normalization_heads AS head
            JOIN LATERAL (
                SELECT revision, generation
                FROM product_listing_raw_revisions
                WHERE product_listing_raw_stream_id = head.product_listing_raw_stream_id
                  AND generation < $2
                ORDER BY generation DESC
                LIMIT 1
            ) AS revision ON TRUE
            WHERE head.product_listing_id = $1
            ON CONFLICT (product_listing_id, product_listing_raw_stream_id) DO UPDATE
            SET last_capture_revision = EXCLUDED.last_capture_revision,
                last_capture_generation = EXCLUDED.last_capture_generation
            WHERE product_listing_auction_override_floors.last_capture_generation
                < EXCLUDED.last_capture_generation
            "#,
        )
        .bind(product_listing_id.as_uuid())
        .bind(generation)
        .execute(&mut *self.connection)
        .await
        .map_err(persistence)?;

        override_from_row(row)
    }
}

fn version_to_i64(
    value: ProductListingAuctionPolicyVersion,
) -> Result<i64, ProductListingAuctionOverrideError> {
    i64::try_from(value.into_inner()).map_err(|_| invalid("policy version exceeds storage range"))
}

fn override_from_row(
    row: OverrideRow,
) -> Result<ProductListingAuctionOverride, ProductListingAuctionOverrideError> {
    let version = u64::try_from(row.policy_version)
        .ok()
        .filter(|value| *value >= 1)
        .ok_or_else(|| invalid("policy version is invalid"))?;
    let release_capture_generation_fence = row
        .release_capture_generation_fence
        .map(raw_capture_generation_from_i64)
        .transpose()?;
    Ok(ProductListingAuctionOverride {
        version: ProductListingAuctionPolicyVersion::from(version),
        active: row.active,
        release_capture_generation_fence,
    })
}

fn raw_capture_revision_to_i64(value: u64) -> Result<i64, ProductListingAuctionOverrideError> {
    let value =
        i64::try_from(value).map_err(|_| invalid("raw capture revision exceeds storage range"))?;
    if value < 1 {
        return Err(invalid("raw capture revision is invalid"));
    }
    Ok(value)
}

fn raw_capture_revision_from_i64(value: i64) -> Result<u64, ProductListingAuctionOverrideError> {
    if value < 1 {
        return Err(invalid("raw capture revision is invalid"));
    }
    u64::try_from(value).map_err(|_| invalid("raw capture revision is invalid"))
}

fn raw_capture_generation_to_i64(value: u64) -> Result<i64, ProductListingAuctionOverrideError> {
    i64::try_from(value).map_err(|_| invalid("raw capture generation exceeds storage range"))
}

fn raw_capture_generation_from_i64(value: i64) -> Result<u64, ProductListingAuctionOverrideError> {
    u64::try_from(value).map_err(|_| invalid("raw capture generation is invalid"))
}

fn persistence(error: sqlx::Error) -> ProductListingAuctionOverrideError {
    ProductListingAuctionOverrideError::Persistence {
        source: box_error(error),
    }
}

fn invalid(message: &'static str) -> ProductListingAuctionOverrideError {
    ProductListingAuctionOverrideError::InvalidPersistedState {
        source: box_error(std::io::Error::other(message)),
    }
}

#[cfg(test)]
mod tests {
    use crate::{
        SqlxProductListingAuctionOverrideRepositoryFactory,
        SqlxProductListingRawCaptureWriterFactory,
    };
    use application::transaction::{Transaction, UnitOfWork};
    use domain_primitives::event_id::EventId;
    use listing_source_core::ListingSourceId;
    use platform_postgres::SqlxUnitOfWork;
    use product_listing_core::product_listing_id::ProductListingId;
    use product_listing_normalization::{
        NormalizationContext, ProductListingNormalizationInput, ProductListingRawValues,
        ProductListingRawValuesNormalizationOutcome, ProductListingRawValuesNormalizer,
        ProductListingRawValuesPatch, ProductListingRawValuesPriceFormat,
        RawProductListingOperation, RawProductListingPayloadFormat, RawProductListingProvenance,
        RawProductListingValues, SourcePayload,
    };
    use product_listing_service::ports::{
        ProductListingAuctionOverride, ProductListingAuctionOverrideAudit,
        ProductListingAuctionOverrideError, ProductListingAuctionOverrideRepository,
        ProductListingAuctionOverrideRepositoryFactory, ProductListingAuctionPolicyVersion,
        ProductListingRawAuctionCapture, ProductListingRawAuctionContextAdmission,
        ProductListingRawCaptureWrite, ProductListingRawCaptureWriteOutcome,
        ProductListingRawCaptureWriter, ProductListingRawCaptureWriterFactory,
        ProductListingRawIngestionMethod, ProductListingRawStreamId, SourceRecordKeySha256,
    };
    use serde_json::json;
    use sha2::{Digest, Sha256};
    use test_api::{IntegrationTestService, Postgres, aura_integration_test, get_postgres_client};
    use time::OffsetDateTime;
    use tokio::sync::oneshot;

    const BUSINESS_SCHEMA: Postgres = Postgres::new("migrations");

    #[aura_integration_test(services = [BUSINESS_SCHEMA])]
    async fn should_block_a_second_policy_decision_until_the_first_transaction_commits() {
        let pool = get_postgres_client().await;
        let (_, listing_id) = seed_listing(&pool).await;
        let unit_of_work = SqlxUnitOfWork::new(pool.clone());
        let factory = SqlxProductListingAuctionOverrideRepositoryFactory::new();
        let mut first_tx = unit_of_work
            .begin()
            .await
            .unwrap_or_else(|error| panic!("begin first policy lock: {error:?}"));
        factory
            .in_transaction(&mut first_tx)
            .lock(listing_id)
            .await
            .unwrap_or_else(|error| panic!("acquire first policy lock: {error:?}"));

        let (acquired, mut wait_for_acquisition) = oneshot::channel();
        let second_pool = pool.clone();
        let second = tokio::spawn(async move {
            let unit_of_work = SqlxUnitOfWork::new(second_pool);
            let factory = SqlxProductListingAuctionOverrideRepositoryFactory::new();
            let mut tx = unit_of_work
                .begin()
                .await
                .map_err(|error| format!("begin second policy lock: {error:?}"))?;
            factory
                .in_transaction(&mut tx)
                .lock(listing_id)
                .await
                .map_err(|error| format!("acquire second policy lock: {error:?}"))?;
            acquired
                .send(())
                .map_err(|_| "test stopped waiting for second policy lock".to_owned())?;
            tx.commit()
                .await
                .map_err(|error| format!("commit second policy lock: {error:?}"))
        });

        assert!(
            tokio::time::timeout(
                std::time::Duration::from_millis(100),
                &mut wait_for_acquisition
            )
            .await
            .is_err(),
            "the second policy decision must wait for the first transaction"
        );
        first_tx
            .commit()
            .await
            .unwrap_or_else(|error| panic!("commit first policy lock: {error:?}"));
        wait_for_acquisition
            .await
            .unwrap_or_else(|_| panic!("second policy lock sender dropped"));
        second
            .await
            .unwrap_or_else(|error| panic!("second policy lock task failed: {error:?}"))
            .unwrap_or_else(|error| panic!("second policy lock transaction failed: {error}"));
    }

    #[aura_integration_test(services = [BUSINESS_SCHEMA])]
    async fn should_activate_version_one_when_auction_override_is_absent() {
        let pool = get_postgres_client().await;
        let (_, listing_id) = seed_listing(&pool).await;
        let audit = correction_audit(listing_id);
        let unit_of_work = SqlxUnitOfWork::new(pool.clone());
        let factory = SqlxProductListingAuctionOverrideRepositoryFactory::new();
        let mut tx = unit_of_work
            .begin()
            .await
            .unwrap_or_else(|error| panic!("begin activation: {error:?}"));
        assert_eq!(
            None,
            factory
                .in_transaction(&mut tx)
                .find(listing_id)
                .await
                .unwrap_or_else(|error| panic!("find absent override: {error:?}")),
        );
        let policy = factory
            .in_transaction(&mut tx)
            .activate(&audit, ProductListingAuctionPolicyVersion::default())
            .await
            .unwrap_or_else(|error| panic!("activate absent override should succeed: {error:?}"));
        assert_eq!(
            ProductListingAuctionOverride {
                version: 1.into(),
                active: true,
                release_capture_generation_fence: None,
            },
            policy
        );
        tx.commit()
            .await
            .unwrap_or_else(|error| panic!("commit activation: {error:?}"));

        assert_persisted_policy(&pool, listing_id, policy).await;
        assert_correction_audit(&pool, &audit).await;
    }

    #[aura_integration_test(services = [BUSINESS_SCHEMA])]
    async fn should_reactivate_and_increment_version_when_auction_override_was_released() {
        let pool = get_postgres_client().await;
        let (_, listing_id) = seed_listing(&pool).await;
        // Seed independently: this regression must reach UPDATE even when first activation fails.
        seed_policy(&pool, listing_id, false, Some(7)).await;
        let audit = correction_audit(listing_id);
        let unit_of_work = SqlxUnitOfWork::new(pool.clone());
        let factory = SqlxProductListingAuctionOverrideRepositoryFactory::new();
        let mut tx = unit_of_work
            .begin()
            .await
            .unwrap_or_else(|error| panic!("begin reactivation: {error:?}"));
        assert_eq!(
            Some(ProductListingAuctionOverride {
                version: 2.into(),
                active: false,
                release_capture_generation_fence: Some(7),
            }),
            factory
                .in_transaction(&mut tx)
                .find(listing_id)
                .await
                .unwrap_or_else(|error| panic!("find released override: {error:?}"))
        );
        let policy = factory
            .in_transaction(&mut tx)
            .activate(&audit, 2.into())
            .await
            .unwrap_or_else(|error| {
                panic!("reactivate existing override should succeed: {error:?}")
            });
        assert_eq!(
            ProductListingAuctionOverride {
                version: 3.into(),
                active: true,
                release_capture_generation_fence: Some(7),
            },
            policy
        );
        tx.commit()
            .await
            .unwrap_or_else(|error| panic!("commit reactivation: {error:?}"));

        assert_persisted_policy(&pool, listing_id, policy).await;
        assert_correction_audit(&pool, &audit).await;
    }

    #[aura_integration_test(services = [BUSINESS_SCHEMA])]
    async fn should_store_capture_and_stream_floors_when_releasing_override_with_linked_raw_stream()
    {
        let pool = get_postgres_client().await;
        let (source_id, listing_id) = seed_listing(&pool).await;
        seed_policy(&pool, listing_id, true, None).await;
        let stream_id = capture_revision(&pool, source_id, 1).await;
        assert_eq!(stream_id, capture_revision(&pool, source_id, 2).await);
        // Only revision one is processed; the release floor must include pending capture two.
        sqlx::query(
            "INSERT INTO product_listing_raw_normalization_heads \
             (product_listing_raw_stream_id, last_processed_revision, product_listing_id, source_listing_id) \
             VALUES ($1, 1, $2, 'override-lot')",
        )
        .bind(stream_id.as_uuid()).bind(listing_id.as_uuid()).execute(&pool).await
        .unwrap_or_else(|error| panic!("link raw stream fixture: {error:?}"));
        let generation: i64 = sqlx::query_scalar(
            "SELECT generation FROM product_listing_raw_revisions \
             WHERE product_listing_raw_stream_id = $1 AND revision = 2",
        )
        .bind(stream_id.as_uuid())
        .fetch_one(&pool)
        .await
        .unwrap_or_else(|error| panic!("read latest capture generation: {error:?}"));
        let release_audit_id = EventId::new();
        let recorded_at = OffsetDateTime::from_unix_timestamp(1_800_000_000)
            .unwrap_or_else(|error| panic!("release timestamp fixture: {error:?}"));
        let unit_of_work = SqlxUnitOfWork::new(pool.clone());
        let factory = SqlxProductListingAuctionOverrideRepositoryFactory::new();
        let mut tx = unit_of_work
            .begin()
            .await
            .unwrap_or_else(|error| panic!("begin release: {error:?}"));
        let policy = factory
            .in_transaction(&mut tx)
            .release(
                listing_id,
                2.into(),
                release_audit_id,
                "regression-operator".to_owned(),
                recorded_at,
            )
            .await
            .unwrap_or_else(|error| panic!("release linked override should succeed: {error:?}"));
        assert_eq!(3, policy.version.into_inner());
        assert!(!policy.active);
        let fence = policy
            .release_capture_generation_fence
            .unwrap_or_else(|| panic!("release must reserve a capture fence"));
        assert!(
            fence
                > u64::try_from(generation).unwrap_or_else(|error| panic!(
                    "capture generation must be nonnegative: {error:?}"
                ))
        );
        tx.commit()
            .await
            .unwrap_or_else(|error| panic!("commit release: {error:?}"));

        assert_persisted_policy(&pool, listing_id, policy).await;
        let floors: Vec<(uuid::Uuid, i64, i64)> = sqlx::query_as(
            "SELECT product_listing_raw_stream_id, last_capture_revision, last_capture_generation \
             FROM product_listing_auction_override_floors WHERE product_listing_id = $1",
        )
        .bind(listing_id.as_uuid())
        .fetch_all(&pool)
        .await
        .unwrap_or_else(|error| panic!("read committed release floors: {error:?}"));
        assert_eq!(vec![(*stream_id.as_uuid(), 2, generation)], floors);
        let release: (uuid::Uuid, String, OffsetDateTime, i64) = sqlx::query_as(
            "SELECT audit_id, actor_label, recorded_at, capture_generation_fence \
             FROM product_listing_auction_override_releases WHERE product_listing_id = $1",
        )
        .bind(listing_id.as_uuid())
        .fetch_one(&pool)
        .await
        .unwrap_or_else(|error| panic!("read committed release audit: {error:?}"));
        assert_eq!(
            (
                *release_audit_id.as_uuid(),
                "regression-operator".to_owned(),
                recorded_at,
                i64::try_from(fence)
                    .unwrap_or_else(|error| panic!("capture fence must fit storage: {error:?}"))
            ),
            release
        );
        let stored_audit_id: uuid::Uuid = sqlx::query_scalar(
            "SELECT released_audit_id FROM product_listing_auction_overrides WHERE product_listing_id = $1",
        )
        .bind(listing_id.as_uuid()).fetch_one(&pool).await
        .unwrap_or_else(|error| panic!("read release audit link: {error:?}"));
        assert_eq!(*release_audit_id.as_uuid(), stored_audit_id);
    }

    #[aura_integration_test(services = [BUSINESS_SCHEMA])]
    async fn should_reserve_release_fence_after_an_uncommitted_raw_capture_allocation() {
        let pool = get_postgres_client().await;
        let (source_id, listing_id) = seed_listing(&pool).await;
        seed_policy(&pool, listing_id, true, None).await;
        let (capture_ready, capture_ready_wait) = oneshot::channel();
        let (allow_capture_commit, allow_capture_commit_wait) = oneshot::channel();
        let capture_pool = pool.clone();
        let capture = tokio::spawn(async move {
            let unit_of_work = SqlxUnitOfWork::new(capture_pool);
            let factory = SqlxProductListingRawCaptureWriterFactory::new();
            let mut tx = unit_of_work
                .begin()
                .await
                .map_err(|error| format!("begin uncommitted raw capture: {error:?}"))?;
            let outcome = factory
                .in_transaction(&mut tx)
                .capture(raw_capture_write(source_id, 1))
                .await
                .map_err(|error| format!("write uncommitted raw capture: {error:?}"))?;
            let ProductListingRawCaptureWriteOutcome::Changed {
                product_listing_raw_stream_id,
                ..
            } = outcome
            else {
                return Err(format!(
                    "expected changed uncommitted raw capture, got {outcome:?}"
                ));
            };
            capture_ready
                .send(product_listing_raw_stream_id)
                .map_err(|_| "test stopped waiting for raw allocation".to_owned())?;
            allow_capture_commit_wait
                .await
                .map_err(|_| "test stopped before raw capture commit".to_owned())?;
            tx.commit()
                .await
                .map_err(|error| format!("commit uncommitted raw capture: {error:?}"))
        });
        let stream_id = capture_ready_wait
            .await
            .unwrap_or_else(|_| panic!("raw capture allocation sender dropped"));

        let unit_of_work = SqlxUnitOfWork::new(pool.clone());
        let factory = SqlxProductListingAuctionOverrideRepositoryFactory::new();
        let mut release_tx = unit_of_work
            .begin()
            .await
            .unwrap_or_else(|error| panic!("begin release during raw capture: {error:?}"));
        factory
            .in_transaction(&mut release_tx)
            .lock(listing_id)
            .await
            .unwrap_or_else(|error| panic!("lock release during raw capture: {error:?}"));
        let policy = factory
            .in_transaction(&mut release_tx)
            .release(
                listing_id,
                2.into(),
                EventId::new(),
                "regression-operator".to_owned(),
                OffsetDateTime::now_utc(),
            )
            .await
            .unwrap_or_else(|error| panic!("release during raw capture: {error:?}"));
        release_tx
            .commit()
            .await
            .unwrap_or_else(|error| panic!("commit release during raw capture: {error:?}"));
        allow_capture_commit
            .send(())
            .unwrap_or_else(|_| panic!("uncommitted raw capture receiver dropped"));
        capture
            .await
            .unwrap_or_else(|error| panic!("raw capture task failed: {error:?}"))
            .unwrap_or_else(|error| panic!("raw capture transaction failed: {error}"));

        let (revision, generation): (i64, i64) = sqlx::query_as(
            "SELECT revision, generation FROM product_listing_raw_revisions \
             WHERE product_listing_raw_stream_id = $1",
        )
        .bind(stream_id.as_uuid())
        .fetch_one(&pool)
        .await
        .unwrap_or_else(|error| panic!("read committed raw capture identity: {error:?}"));
        let fence = policy
            .release_capture_generation_fence
            .unwrap_or_else(|| panic!("release must reserve a capture fence"));
        assert!(
            fence
                > u64::try_from(generation).unwrap_or_else(|error| panic!(
                    "raw generation must be nonnegative: {error:?}"
                ))
        );

        let mut admission_tx = unit_of_work
            .begin()
            .await
            .unwrap_or_else(|error| panic!("begin uncommitted-capture admission: {error:?}"));
        factory
            .in_transaction(&mut admission_tx)
            .lock(listing_id)
            .await
            .unwrap_or_else(|error| panic!("lock uncommitted-capture admission: {error:?}"));
        assert_eq!(
            ProductListingRawAuctionContextAdmission::Preserve,
            factory
                .in_transaction(&mut admission_tx)
                .admit_raw_auction_context(
                    listing_id,
                    ProductListingRawAuctionCapture {
                        product_listing_raw_stream_id: stream_id,
                        revision: u64::try_from(revision).unwrap_or_else(|error| {
                            panic!("raw revision must be positive: {error:?}")
                        }),
                        capture_generation: u64::try_from(generation).unwrap_or_else(|error| {
                            panic!("raw generation must be nonnegative: {error:?}")
                        }),
                    },
                )
                .await
                .unwrap_or_else(|error| panic!("admit pre-fence raw capture: {error:?}"))
        );
    }

    #[aura_integration_test(services = [BUSINESS_SCHEMA])]
    async fn should_enforce_release_fence_and_linked_stream_floor_on_raw_auction_admission() {
        let pool = get_postgres_client().await;
        let (source_id, listing_id) = seed_listing(&pool).await;
        let stream_id = capture_revision(&pool, source_id, 1).await;
        assert_eq!(stream_id, capture_revision(&pool, source_id, 2).await);
        let captures: Vec<(i64, i64)> = sqlx::query_as(
            "SELECT revision, generation FROM product_listing_raw_revisions \
             WHERE product_listing_raw_stream_id = $1 ORDER BY revision",
        )
        .bind(stream_id.as_uuid())
        .fetch_all(&pool)
        .await
        .unwrap_or_else(|error| panic!("read raw capture identities: {error:?}"));
        let [
            (first_revision, first_generation),
            (second_revision, second_generation),
        ] = captures.as_slice()
        else {
            panic!("expected two raw capture identities");
        };
        seed_policy(&pool, listing_id, false, Some(*first_generation)).await;

        let unit_of_work = SqlxUnitOfWork::new(pool.clone());
        let factory = SqlxProductListingAuctionOverrideRepositoryFactory::new();
        let mut tx = unit_of_work
            .begin()
            .await
            .unwrap_or_else(|error| panic!("begin raw admission: {error:?}"));
        factory
            .in_transaction(&mut tx)
            .lock(listing_id)
            .await
            .unwrap_or_else(|error| panic!("lock raw admission: {error:?}"));
        let first = ProductListingRawAuctionCapture {
            product_listing_raw_stream_id: stream_id,
            revision: u64::try_from(*first_revision)
                .unwrap_or_else(|error| panic!("first revision must be valid: {error:?}")),
            capture_generation: u64::try_from(*first_generation)
                .unwrap_or_else(|error| panic!("first generation must be valid: {error:?}")),
        };
        let second = ProductListingRawAuctionCapture {
            product_listing_raw_stream_id: stream_id,
            revision: u64::try_from(*second_revision)
                .unwrap_or_else(|error| panic!("second revision must be valid: {error:?}")),
            capture_generation: u64::try_from(*second_generation)
                .unwrap_or_else(|error| panic!("second generation must be valid: {error:?}")),
        };
        assert_eq!(
            ProductListingRawAuctionContextAdmission::Preserve,
            factory
                .in_transaction(&mut tx)
                .admit_raw_auction_context(listing_id, first)
                .await
                .unwrap_or_else(|error| panic!("admit pre-fence capture: {error:?}"))
        );
        assert_eq!(
            ProductListingRawAuctionContextAdmission::Admit,
            factory
                .in_transaction(&mut tx)
                .admit_raw_auction_context(listing_id, second)
                .await
                .unwrap_or_else(|error| panic!("admit post-fence capture: {error:?}"))
        );
        tx.commit()
            .await
            .unwrap_or_else(|error| panic!("commit raw admission: {error:?}"));

        sqlx::query(
            "INSERT INTO product_listing_auction_override_floors \
             (product_listing_id, product_listing_raw_stream_id, last_capture_revision, last_capture_generation) \
             VALUES ($1, $2, $3, $4)",
        )
        .bind(listing_id.as_uuid())
        .bind(stream_id.as_uuid())
        .bind(second_revision)
        .bind(second_generation)
        .execute(&pool)
        .await
        .unwrap_or_else(|error| panic!("seed linked stream floor: {error:?}"));
        let mut tx = unit_of_work
            .begin()
            .await
            .unwrap_or_else(|error| panic!("begin floor admission: {error:?}"));
        factory
            .in_transaction(&mut tx)
            .lock(listing_id)
            .await
            .unwrap_or_else(|error| panic!("lock floor admission: {error:?}"));
        assert_eq!(
            ProductListingRawAuctionContextAdmission::Preserve,
            factory
                .in_transaction(&mut tx)
                .admit_raw_auction_context(listing_id, second)
                .await
                .unwrap_or_else(|error| panic!("admit floored capture: {error:?}"))
        );
        tx.commit()
            .await
            .unwrap_or_else(|error| panic!("commit floor admission: {error:?}"));

        sqlx::query(
            "UPDATE product_listing_auction_override_floors SET last_capture_generation = $1 \
             WHERE product_listing_id = $2 AND product_listing_raw_stream_id = $3",
        )
        .bind(first_generation)
        .bind(listing_id.as_uuid())
        .bind(stream_id.as_uuid())
        .execute(&pool)
        .await
        .unwrap_or_else(|error| panic!("corrupt linked stream floor: {error:?}"));
        let mut tx = unit_of_work
            .begin()
            .await
            .unwrap_or_else(|error| panic!("begin corrupt-floor admission: {error:?}"));
        factory
            .in_transaction(&mut tx)
            .lock(listing_id)
            .await
            .unwrap_or_else(|error| panic!("lock corrupt-floor admission: {error:?}"));
        assert!(matches!(
            factory
                .in_transaction(&mut tx)
                .admit_raw_auction_context(listing_id, second)
                .await,
            Err(ProductListingAuctionOverrideError::InvalidPersistedState { .. })
        ));
    }

    async fn assert_persisted_policy(
        pool: &sqlx::PgPool,
        listing_id: ProductListingId,
        expected: ProductListingAuctionOverride,
    ) {
        let unit_of_work = SqlxUnitOfWork::new(pool.clone());
        let factory = SqlxProductListingAuctionOverrideRepositoryFactory::new();
        let mut tx = unit_of_work
            .begin()
            .await
            .unwrap_or_else(|error| panic!("begin policy read: {error:?}"));
        assert_eq!(
            Some(expected),
            factory
                .in_transaction(&mut tx)
                .find(listing_id)
                .await
                .unwrap_or_else(|error| panic!("read committed policy: {error:?}"))
        );
        tx.commit()
            .await
            .unwrap_or_else(|error| panic!("commit policy read: {error:?}"));
    }

    fn correction_audit(listing_id: ProductListingId) -> ProductListingAuctionOverrideAudit {
        ProductListingAuctionOverrideAudit {
            audit_id: EventId::new(),
            product_listing_id: listing_id,
            actor_label: "regression-operator".to_owned(),
            reason: "Detach incorrect source auction".to_owned(),
            previous_auction_id: None,
            current_auction_id: None,
            recorded_at: OffsetDateTime::from_unix_timestamp(1_800_000_000)
                .unwrap_or_else(|error| panic!("correction timestamp fixture: {error:?}")),
        }
    }

    async fn assert_correction_audit(
        pool: &sqlx::PgPool,
        audit: &ProductListingAuctionOverrideAudit,
    ) {
        let stored: (uuid::Uuid, String, String, Option<uuid::Uuid>, Option<uuid::Uuid>, OffsetDateTime) = sqlx::query_as(
            "SELECT correction.audit_id, correction.actor_label, correction.reason, \
                    correction.previous_auction_id, correction.current_auction_id, correction.recorded_at \
             FROM product_listing_auction_corrections correction \
             JOIN product_listing_auction_overrides policy ON policy.correction_audit_id = correction.audit_id \
             WHERE policy.product_listing_id = $1",
        )
        .bind(audit.product_listing_id.as_uuid()).fetch_one(pool).await
        .unwrap_or_else(|error| panic!("read committed correction audit: {error:?}"));
        assert_eq!(
            (
                *audit.audit_id.as_uuid(),
                audit.actor_label.clone(),
                audit.reason.clone(),
                audit.previous_auction_id.map(|id| *id.as_uuid()),
                audit.current_auction_id.map(|id| *id.as_uuid()),
                audit.recorded_at,
            ),
            stored
        );
    }

    async fn seed_policy(
        pool: &sqlx::PgPool,
        listing_id: ProductListingId,
        active: bool,
        generation: Option<i64>,
    ) {
        sqlx::query(
            "INSERT INTO product_listing_auction_overrides \
             (product_listing_id, policy_version, active, release_capture_generation_fence) VALUES ($1, 2, $2, $3)",
        )
        .bind(listing_id.as_uuid()).bind(active).bind(generation).execute(pool).await
        .unwrap_or_else(|error| panic!("seed existing override policy: {error:?}"));
    }

    async fn seed_listing(pool: &sqlx::PgPool) -> (ListingSourceId, ProductListingId) {
        let source_id = ListingSourceId::new();
        let party_id = uuid::Uuid::now_v7();
        sqlx::query("INSERT INTO parties (party_id, party_slug_id, name) VALUES ($1, 'override-party', 'Override party')")
            .bind(party_id).execute(pool).await
            .unwrap_or_else(|error| panic!("seed listing-source party: {error:?}"));
        sqlx::query("INSERT INTO listing_sources (listing_source_id, listing_source_slug_id, name, operator_party_id) VALUES ($1, 'override-source', 'Override source', $2)")
            .bind(source_id.as_uuid()).bind(party_id).execute(pool).await
            .unwrap_or_else(|error| panic!("seed listing source: {error:?}"));
        let listing_id = ProductListingId::new();
        let event_id = EventId::new();
        let mut tx = pool
            .begin()
            .await
            .unwrap_or_else(|error| panic!("begin listing fixture: {error:?}"));
        sqlx::query(
            "INSERT INTO product_listings \
             (product_listing_id, product_listing_title_slug_id, current_event_id, content_source_event_id, \
              embedding_source_event_id, listing_source_id, source_listing_id, title_text, title_language, \
              availability, lifecycle, url, product_images) \
             VALUES ($1, 'override-lot-000001', $2, $2, $2, $3, 'override-lot', 'Override lot', 'en', \
                     'AVAILABLE', 'ACTIVE', 'https://example.test/override-lot', '[]')",
        )
        .bind(listing_id.as_uuid()).bind(event_id.as_uuid()).bind(source_id.as_uuid())
        .execute(&mut *tx).await.unwrap_or_else(|error| panic!("seed listing: {error:?}"));
        let discovered = json!({
            "listingSourceId": source_id.as_uuid().to_string(),
            "sourceListingId": "override-lot",
            "title": {"language": "en", "text": "Override lot"},
            "description": null,
            "pricing": {"price": null, "priceEstimateMin": null, "priceEstimateMax": null},
            "availability": "AVAILABLE",
            "url": "https://example.test/override-lot",
            "imageCount": 0,
            "auction": null
        });
        crate::product_listing_event_codec::decode("PRODUCT_LISTING_DISCOVERED", 1, &discovered)
            .unwrap_or_else(|error| panic!("validate discovered event fixture: {error:?}"));
        sqlx::query(
            "INSERT INTO product_listing_events \
             (event_id, product_listing_id, event_type, event_group, event_type_schema_version, payload, event_time) \
             VALUES ($1, $2, 'PRODUCT_LISTING_DISCOVERED', 'DOMAIN', 1, $3, now())",
        )
        .bind(event_id.as_uuid()).bind(listing_id.as_uuid()).bind(discovered).execute(&mut *tx).await
        .unwrap_or_else(|error| panic!("seed listing event: {error:?}"));
        tx.commit()
            .await
            .unwrap_or_else(|error| panic!("commit listing fixture: {error:?}"));
        (source_id, listing_id)
    }

    async fn capture_revision(
        pool: &sqlx::PgPool,
        source_id: ListingSourceId,
        revision: u64,
    ) -> ProductListingRawStreamId {
        let write = raw_capture_write(source_id, revision);
        let unit_of_work = SqlxUnitOfWork::new(pool.clone());
        let factory = SqlxProductListingRawCaptureWriterFactory::new();
        let mut tx = unit_of_work
            .begin()
            .await
            .unwrap_or_else(|error| panic!("begin raw capture: {error:?}"));
        let outcome = factory
            .in_transaction(&mut tx)
            .capture(write)
            .await
            .unwrap_or_else(|error| panic!("capture raw fixture: {error:?}"));
        tx.commit()
            .await
            .unwrap_or_else(|error| panic!("commit raw capture: {error:?}"));
        match outcome {
            ProductListingRawCaptureWriteOutcome::Changed {
                product_listing_raw_stream_id,
                revision: captured_revision,
                ..
            } => {
                assert_eq!(revision, captured_revision);
                product_listing_raw_stream_id
            }
            other => panic!("expected changed raw capture, got {other:?}"),
        }
    }

    fn raw_capture_write(
        source_id: ListingSourceId,
        revision: u64,
    ) -> ProductListingRawCaptureWrite {
        use ProductListingRawValuesPatch::{Clear, Set, Unchanged};

        let raw_values = ProductListingRawValues {
            source_listing_id: "override-lot".to_owned(),
            title: Set("Override lot".to_owned()),
            description: Clear,
            price_format: ProductListingRawValuesPriceFormat::DisplayText,
            price: if revision == 1 {
                Clear
            } else {
                Set("EUR 120".to_owned())
            },
            price_estimate_min: Clear,
            price_estimate_max: Clear,
            availability: Set("in stock".to_owned()),
            url: Set("https://example.test/override-lot".to_owned()),
            images: Set(Vec::new()),
            auction: Unchanged,
            attributes: Default::default(),
        };
        let input = ProductListingNormalizationInput::new(
            RawProductListingOperation::Upsert,
            RawProductListingPayloadFormat::CrawlerExtractedProduct,
            1,
            1,
            SourcePayload::new(json!({"capture": revision}))
                .unwrap_or_else(|error| panic!("source payload fixture: {error:?}")),
            RawProductListingValues::new(
                serde_json::to_value(raw_values)
                    .unwrap_or_else(|error| panic!("serialize raw values fixture: {error:?}")),
            )
            .unwrap_or_else(|error| panic!("raw values fixture: {error:?}")),
            NormalizationContext::new(json!({
                "baseUrl": "https://example.test/override-lot",
                "fallbackCurrency": "EUR",
                "fallbackLanguage": "en"
            }))
            .unwrap_or_else(|error| panic!("normalization context fixture: {error:?}")),
        )
        .unwrap_or_else(|error| panic!("normalization input fixture: {error:?}"));
        match ProductListingRawValuesNormalizer::new().normalize(&input) {
            ProductListingRawValuesNormalizationOutcome::Resolved(_) => {}
            other => panic!("raw fixture should normalize successfully: {other:?}"),
        }
        ProductListingRawCaptureWrite {
            listing_source_id: source_id,
            ingestion_method: ProductListingRawIngestionMethod::WebCrawl,
            source_record_key: "override-lot".to_owned(),
            source_record_key_sha256: SourceRecordKeySha256::new(
                Sha256::digest(b"override-lot").into(),
            ),
            input_sha256: input
                .hash()
                .unwrap_or_else(|error| panic!("hash raw fixture: {error:?}")),
            input,
            provenance: RawProductListingProvenance::new(json!({"capture": revision}))
                .unwrap_or_else(|error| panic!("raw provenance fixture: {error:?}")),
            source_event_id: None,
            source_occurred_at: None,
            provider_receipt: None,
        }
    }
}

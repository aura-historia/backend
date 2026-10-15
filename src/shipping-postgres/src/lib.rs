pub mod assessment;
mod terms;
use application::error::{BoxError, box_error};
use listing_source_core::ListingSourceId;
use party_core::party_id::PartyId;
use platform_postgres::SqlxTransaction;
use serde::{Deserialize, Serialize};
use shipping_core::{
    profile::{
        ShippingProfile, ShippingProfileContent, ShippingProfileId, ShippingProfileLifecycle,
        ShippingProfileName, ShippingProfileRevision,
    },
    selection::SourceDefaultProfile,
    terms::{DeliverySite, ShippingPolicy},
};
use shipping_service::{
    ShippingError,
    ports::{
        ShippingDefaultRevision, ShippingEvidence, ShippingManagementRevision, ShippingReader,
        ShippingReaderFactory, ShippingRepository, ShippingRepositoryFactory, StoredDefault,
        StoredProfile,
    },
    use_cases::{DefaultEligibility, DefaultView, ProfileSummary, ProfileView},
};
use sqlx::{FromRow, PgConnection};
use uuid::Uuid;

#[derive(Debug, Clone, Copy, Default)]
pub struct SqlxShippingRepositoryFactory;
#[derive(Debug, Clone, Copy, Default)]
pub struct SqlxShippingReaderFactory;
struct Repository<'a>(&'a mut PgConnection);
struct Reader<'a>(&'a mut PgConnection);
impl ShippingRepositoryFactory<SqlxTransaction> for SqlxShippingRepositoryFactory {
    fn in_transaction<'tx>(
        &'tx self,
        tx: &'tx mut SqlxTransaction,
    ) -> impl ShippingRepository + 'tx {
        Repository(tx.connection())
    }
}
impl ShippingReaderFactory<SqlxTransaction> for SqlxShippingReaderFactory {
    fn in_transaction<'tx>(&'tx self, tx: &'tx mut SqlxTransaction) -> impl ShippingReader + 'tx {
        Reader(tx.connection())
    }
}
fn sql_error(source: sqlx::Error) -> ShippingError {
    if let sqlx::Error::Database(ref error) = source
        && error.is_foreign_key_violation()
        && error.constraint() == Some("shipping_profiles_attributed_party_id_fkey")
    {
        return ShippingError::AttributedPartyNotFound {
            source: box_error(source),
        };
    }
    if let sqlx::Error::Database(ref e) = source
        && (e.is_check_violation() || e.is_foreign_key_violation() || e.is_unique_violation())
    {
        ShippingError::PersistenceFailed {
            source: box_error(source),
        }
    } else {
        ShippingError::PersistenceUnavailable {
            source: box_error(source),
        }
    }
}
fn invalid(source: impl std::error::Error + Send + Sync + 'static) -> ShippingError {
    ShippingError::CorruptShippingState {
        source: box_error(source),
    }
}
fn corrupt() -> ShippingError {
    invalid(std::io::Error::other("invalid shipping storage state"))
}
fn corrupt_box(source: BoxError) -> ShippingError {
    ShippingError::CorruptShippingState { source }
}
fn encode_error(source: BoxError) -> ShippingError {
    ShippingError::PersistenceFailed { source }
}
fn positive(value: i64) -> Result<u64, ShippingError> {
    if value <= 0 {
        Err(corrupt())
    } else {
        Ok(value as u64)
    }
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct EvidenceData {
    reference: String,
    assertion: String,
}
fn encode_evidence(evidence: &[ShippingEvidence]) -> Result<serde_json::Value, ShippingError> {
    serde_json::to_value(
        evidence
            .iter()
            .map(|e| EvidenceData {
                reference: e.reference().into(),
                assertion: e.assertion().into(),
            })
            .collect::<Vec<_>>(),
    )
    .map_err(|e| encode_error(box_error(e)))
}
fn decode_evidence(value: serde_json::Value) -> Result<Vec<ShippingEvidence>, ShippingError> {
    let data: Vec<EvidenceData> = serde_json::from_value(value).map_err(invalid)?;
    if data.len() > 32 {
        return Err(corrupt());
    }
    let evidence = data
        .into_iter()
        .map(|e| ShippingEvidence::new(e.reference, e.assertion).map_err(invalid))
        .collect::<Result<Vec<_>, _>>()?;
    if evidence
        .iter()
        .enumerate()
        .any(|(i, e)| evidence[..i].contains(e))
    {
        return Err(corrupt());
    }
    Ok(evidence)
}
#[derive(FromRow)]
struct ProfileRow {
    shipping_profile_id: Uuid,
    listing_source_id: Uuid,
    management_revision: i64,
    terms_revision: i64,
    lifecycle: String,
    name: String,
    attributed_party_id: Option<Uuid>,
    terms: serde_json::Value,
}
impl ProfileRow {
    fn view(self) -> Result<ProfileView, ShippingError> {
        let source = ListingSourceId::try_from(self.listing_source_id).map_err(invalid)?;
        let management_revision =
            ShippingManagementRevision::try_from(self.management_revision).map_err(invalid)?;
        let revision =
            ShippingProfileRevision::try_from(positive(self.terms_revision)?).map_err(invalid)?;
        if revision.into_inner() > management_revision.into_inner() {
            return Err(corrupt());
        }
        let profile = ShippingProfile::rehydrate(
            ShippingProfileId::try_from(self.shipping_profile_id).map_err(invalid)?,
            source,
            revision,
            ShippingProfileLifecycle::from_code(&self.lifecycle).ok_or_else(corrupt)?,
            ShippingProfileContent {
                name: ShippingProfileName::new(self.name).map_err(invalid)?,
                attributed_party: self
                    .attributed_party_id
                    .map(PartyId::try_from)
                    .transpose()
                    .map_err(invalid)?,
                terms: terms::decode(self.terms, source).map_err(corrupt_box)?,
            },
        )
        .map_err(invalid)?;
        Ok(ProfileView {
            id: profile.id(),
            source_id: source,
            management_revision,
            revision: profile.revision(),
            lifecycle: profile.lifecycle(),
            content: profile.content().clone(),
        })
    }
}
async fn stored(
    connection: &mut PgConnection,
    row: ProfileRow,
) -> Result<StoredProfile, ShippingError> {
    let view = row.view()?;
    let evidence: serde_json::Value = sqlx::query_scalar(
        r#"
            SELECT evidence FROM shipping_profile_evidence WHERE shipping_profile_id=$1 AND
            management_revision=$2
        "#,
    )
    .bind(view.id.into_uuid())
    .bind(view.management_revision.into_inner() as i64)
    .fetch_optional(&mut *connection)
    .await
    .map_err(sql_error)?
    .ok_or_else(corrupt)?;
    Ok(StoredProfile {
        profile: ShippingProfile::rehydrate(
            view.id,
            view.source_id,
            view.revision,
            view.lifecycle,
            view.content,
        )
        .map_err(invalid)?,
        management_revision: view.management_revision,
        evidence: decode_evidence(evidence)?,
    })
}
async fn record(
    connection: &mut PgConnection,
    p: &StoredProfile,
    actor: &str,
    kind: &str,
) -> Result<(), ShippingError> {
    let id = p.profile.id().into_uuid();
    let management = p.management_revision.into_inner() as i64;
    sqlx::query(
        r#"
            INSERT INTO shipping_profile_history(shipping_profile_id,management_revision,terms_revision,lifecycle,name,attributed_party_id,terms) SELECT
            shipping_profile_id,management_revision,terms_revision,lifecycle,name,attributed_party_id,terms FROM
            shipping_profiles WHERE shipping_profile_id=$1
        "#,
    )
        .bind(id)
        .execute(&mut *connection)
        .await
        .map_err(sql_error)?;
    sqlx::query(
        r#"
            INSERT INTO shipping_profile_evidence(shipping_profile_id,management_revision,evidence,actor)
            VALUES($1,$2,$3,$4)
        "#,
    )
        .bind(id)
        .bind(management)
        .bind(encode_evidence(&p.evidence)?)
        .bind(actor)
        .execute(&mut *connection)
        .await
        .map_err(sql_error)?;
    sqlx::query(
        r#"
            INSERT INTO shipping_changes(listing_source_id,subject_kind,subject_id,revision,terms_revision,change_kind,actor) VALUES($1,'PROFILE',$2,$3,$4,$5,$6)
        "#,
    )
        .bind(p.profile.source_id().into_uuid())
        .bind(id)
        .bind(management)
        .bind(p.profile.revision().into_inner() as i64)
        .bind(kind)
        .bind(actor)
        .execute(&mut *connection)
        .await
        .map_err(sql_error)?;
    sqlx::query(
        r#"
            DELETE FROM shipping_profile_location_references WHERE shipping_profile_id=$1
        "#,
    )
    .bind(id)
    .execute(&mut *connection)
    .await
    .map_err(sql_error)?;
    let t = &p.profile.content().terms;
    let origin = match t.shipping() {
        Some(ShippingPolicy::Coverage(c)) => c.dispatch_origin(),
        _ => None,
    };
    for (kind, site) in [
        ("DISPATCH", origin),
        ("COLLECTION", t.collection().and_then(|c| c.site())),
    ] {
        if let Some(DeliverySite::PartyLocation(location)) = site {
            sqlx::query(
                r#"
            INSERT INTO
            shipping_profile_location_references(shipping_profile_id,party_location_id,applicability)
            VALUES($1,$2,$3)
        "#,
            )
        .bind(id)
        .bind(location.into_uuid())
        .bind(kind)
        .execute(&mut *connection)
        .await
        .map_err(sql_error)?;
        }
    }
    Ok(())
}
#[derive(FromRow)]
struct DefaultRow {
    listing_source_id: Uuid,
    shipping_profile_id: Option<Uuid>,
    revision: i64,
    source_deleted: bool,
}
impl DefaultRow {
    fn stored(self) -> Result<StoredDefault, ShippingError> {
        if self.revision < 2 {
            return Err(corrupt());
        }
        if self.source_deleted {
            return Err(ShippingError::SourceNotFound);
        }
        Ok(StoredDefault {
            selection: SourceDefaultProfile::rehydrate(
                ListingSourceId::try_from(self.listing_source_id).map_err(invalid)?,
                self.shipping_profile_id
                    .map(ShippingProfileId::try_from)
                    .transpose()
                    .map_err(invalid)?,
            ),
            revision: ShippingDefaultRevision::try_from(self.revision).map_err(invalid)?,
        })
    }
}
#[async_trait::async_trait]
impl ShippingRepository for Repository<'_> {
    async fn creation_retry(
        &mut self,
        source: ListingSourceId,
        caller: &str,
        key: &str,
        content: &ShippingProfileContent,
        evidence: &[ShippingEvidence],
    ) -> Result<Option<StoredProfile>, ShippingError> {
        let id: Option<Uuid> = sqlx::query_scalar(
            r#"
            SELECT shipping_profile_id FROM shipping_profile_creation_receipts WHERE listing_source_id=$1 AND
            caller=$2 AND creation_key=$3
        "#,
        )
        .bind(source.into_uuid())
        .bind(caller)
        .bind(key)
        .fetch_optional(&mut *self.0)
        .await
        .map_err(sql_error)?;
        let Some(id) = id else { return Ok(None) };
        let original = sqlx::query_as::<_, ProfileRow>(
            r#"
            SELECT p.shipping_profile_id,p.listing_source_id,h.management_revision,h.terms_revision,h.lifecycle,
            h.name,h.attributed_party_id,h.terms FROM shipping_profiles p JOIN shipping_profile_history h ON
            h.shipping_profile_id=p.shipping_profile_id AND h.management_revision=1 WHERE
            p.shipping_profile_id=$1 AND p.listing_source_id=$2
        "#,
        )
        .bind(id)
        .bind(source.into_uuid())
        .fetch_optional(&mut *self.0)
        .await
        .map_err(sql_error)?
        .ok_or_else(corrupt)?;
        let original = stored(self.0, original).await?;
        if original.profile.content() != content || original.evidence != evidence {
            return Err(ShippingError::CreationKeyConflict);
        }
        self.find_for_update(source, ShippingProfileId::try_from(id).map_err(invalid)?)
            .await
            .map(Some)
    }
    async fn insert(
        &mut self,
        p: &StoredProfile,
        caller: &str,
        key: &str,
    ) -> Result<(), ShippingError> {
        let c = p.profile.content();
        sqlx::query(
            r#"
            INSERT INTO shipping_profiles(shipping_profile_id,listing_source_id,current_source_id,management_revision,terms_revision,lifecycle,name,attributed_party_id,terms) VALUES($1,$2,$2,$3,$4,$5,$6,$7,$8)
        "#,
        )
   .bind(p.profile.id().into_uuid())
        .bind(p.profile.source_id().into_uuid())
        .bind(p.management_revision.into_inner() as i64)
        .bind(p.profile.revision().into_inner() as i64)
        .bind(p.profile.lifecycle().as_str())
        .bind(c.name.as_str())
        .bind(c.attributed_party.map(|v|v.into_uuid()))
        .bind(terms::encode(&c.terms).map_err(encode_error)?)
        .execute(&mut *self.0)
        .await
        .map_err(sql_error)?;
        record(self.0, p, caller, "CREATED").await?;
        sqlx::query(
            r#"
            INSERT INTO
            shipping_profile_creation_receipts(listing_source_id,caller,creation_key,shipping_profile_id)
            VALUES($1,$2,$3,$4)
        "#,
        )
        .bind(p.profile.source_id().into_uuid())
        .bind(caller)
        .bind(key)
        .bind(p.profile.id().into_uuid())
        .execute(&mut *self.0)
        .await
        .map_err(sql_error)?;
        Ok(())
    }
    async fn find_for_update(
        &mut self,
        source: ListingSourceId,
        id: ShippingProfileId,
    ) -> Result<StoredProfile, ShippingError> {
        let row = sqlx::query_as::<_, ProfileRow>(
            r#"
            SELECT shipping_profile_id,listing_source_id,management_revision,terms_revision,lifecycle,name,attributed_party_id,terms FROM shipping_profiles WHERE listing_source_id=$1 AND shipping_profile_id=$2
            FOR UPDATE
        "#,
        )
        .bind(source.into_uuid())
        .bind(id.into_uuid())
        .fetch_optional(&mut *self.0)
        .await
        .map_err(sql_error)?
        .ok_or(ShippingError::ProfileNotFound)?;
        stored(self.0, row).await
    }
    async fn save(
        &mut self,
        p: &StoredProfile,
        expected: ShippingManagementRevision,
        actor: &str,
    ) -> Result<(), ShippingError> {
        let old: (i64, String) = sqlx::query_as(
            r#"
            SELECT terms_revision,lifecycle FROM shipping_profiles WHERE shipping_profile_id=$1 AND
            listing_source_id=$2 AND management_revision=$3
        "#,
        )
        .bind(p.profile.id().into_uuid())
        .bind(p.profile.source_id().into_uuid())
        .bind(expected.into_inner() as i64)
        .fetch_optional(&mut *self.0)
        .await
        .map_err(sql_error)?
        .ok_or(ShippingError::StaleRevision)?;
        let kind = if old.0 == p.profile.revision().into_inner() as i64 {
            "EVIDENCE_CORRECTED"
        } else if old.1 != p.profile.lifecycle().as_str() {
            if p.profile.lifecycle() == ShippingProfileLifecycle::Retired {
                "RETIRED"
            } else {
                "RESTORED"
            }
        } else {
            "REPLACED"
        };
        let c = p.profile.content();
        let changed = sqlx::query(
            r#"
            UPDATE shipping_profiles SET management_revision=$4,terms_revision=$5,lifecycle=$6,name=$7,attributed_party_id=$8,terms=$9,updated=now() WHERE shipping_profile_id=$1 AND listing_source_id=$2 AND
            management_revision=$3
        "#,
        )
   .bind(p.profile.id().into_uuid())
        .bind(p.profile.source_id().into_uuid())
        .bind(expected.into_inner() as i64)
        .bind(p.management_revision.into_inner() as i64)
        .bind(p.profile.revision().into_inner() as i64)
        .bind(p.profile.lifecycle().as_str())
        .bind(c.name.as_str())
        .bind(c.attributed_party.map(|v|v.into_uuid()))
        .bind(terms::encode(&c.terms).map_err(encode_error)?)
        .execute(&mut *self.0)
        .await
        .map_err(sql_error)?.rows_affected();
        if changed != 1 {
            return Err(ShippingError::StaleRevision);
        }
        record(self.0, p, actor, kind).await
    }
    async fn default_for_update(
        &mut self,
        source: ListingSourceId,
    ) -> Result<StoredDefault, ShippingError> {
        let row = sqlx::query_as::<_, DefaultRow>(
            r#"
            SELECT listing_source_id,shipping_profile_id,revision,source_deleted FROM shipping_source_defaults
            WHERE listing_source_id=$1 FOR UPDATE
        "#,
        )
        .bind(source.into_uuid())
        .fetch_optional(&mut *self.0)
        .await
        .map_err(sql_error)?;
        match row {
            Some(row) => row.stored(),
            None => Ok(StoredDefault {
                selection: SourceDefaultProfile::empty(source),
                revision: ShippingDefaultRevision::INITIAL,
            }),
        }
    }
    async fn save_default(
        &mut self,
        d: &StoredDefault,
        expected: ShippingDefaultRevision,
        actor: &str,
    ) -> Result<(), ShippingError> {
        let affected = sqlx::query(
            r#"
            INSERT INTO
            shipping_source_defaults(listing_source_id,current_source_id,shipping_profile_id,revision) SELECT
            $1,$1,$2,$3 WHERE $4=1 OR EXISTS(SELECT 1 FROM shipping_source_defaults WHERE listing_source_id=$1
            AND revision=$4 AND NOT source_deleted) ON CONFLICT(listing_source_id) DO UPDATE SET
            shipping_profile_id=EXCLUDED.shipping_profile_id,revision=EXCLUDED.revision WHERE
            shipping_source_defaults.revision=$4 AND NOT shipping_source_defaults.source_deleted
        "#,
        )
   .bind(d.selection.source_id().into_uuid())
        .bind(d.selection.profile_id().map(|p|p.into_uuid()))
        .bind(d.revision.into_inner() as i64)
        .bind(expected.into_inner() as i64)
        .execute(&mut *self.0)
        .await
        .map_err(sql_error)?.rows_affected();
        if affected != 1 {
            return Err(ShippingError::StaleRevision);
        }
        sqlx::query(
            r#"
            INSERT INTO shipping_changes(listing_source_id,subject_kind,subject_id,revision,change_kind,actor)
            VALUES($1,'SOURCE_DEFAULT',$1,$2,$3,$4)
        "#,
        )
        .bind(d.selection.source_id().into_uuid())
        .bind(d.revision.into_inner() as i64)
        .bind(if d.selection.profile_id().is_some(){"DEFAULT_ASSIGNED"}else{"DEFAULT_CLEARED"})
        .bind(actor)
        .execute(&mut *self.0)
        .await
        .map_err(sql_error)?;
        Ok(())
    }
}
#[derive(FromRow)]
struct SummaryRow {
    shipping_profile_id: Uuid,
    listing_source_id: Uuid,
    name: String,
    management_revision: i64,
    terms_revision: i64,
    lifecycle: String,
}
impl SummaryRow {
    fn view(self) -> Result<ProfileSummary, ShippingError> {
        let revision =
            ShippingProfileRevision::try_from(positive(self.terms_revision)?).map_err(invalid)?;
        let management_revision =
            ShippingManagementRevision::try_from(self.management_revision).map_err(invalid)?;
        if revision.into_inner() > management_revision.into_inner() {
            return Err(corrupt());
        }
        Ok(ProfileSummary {
            id: ShippingProfileId::try_from(self.shipping_profile_id).map_err(invalid)?,
            source_id: ListingSourceId::try_from(self.listing_source_id).map_err(invalid)?,
            name: ShippingProfileName::new(self.name).map_err(invalid)?,
            management_revision,
            revision,
            lifecycle: ShippingProfileLifecycle::from_code(&self.lifecycle).ok_or_else(corrupt)?,
        })
    }
}
#[async_trait::async_trait]
impl ShippingReader for Reader<'_> {
    async fn get(
        &mut self,
        source: ListingSourceId,
        id: ShippingProfileId,
    ) -> Result<ProfileView, ShippingError> {
        sqlx::query_as::<_, ProfileRow>(
            r#"
            SELECT shipping_profile_id,listing_source_id,management_revision,terms_revision,lifecycle,name,attributed_party_id,terms FROM shipping_profiles WHERE listing_source_id=$1 AND shipping_profile_id=$2
        "#,
        )
        .bind(source.into_uuid())
        .bind(id.into_uuid())
        .fetch_optional(&mut *self.0)
        .await
        .map_err(sql_error)?
        .ok_or(ShippingError::ProfileNotFound)?.view()
    }
    async fn list(
        &mut self,
        source: ListingSourceId,
        after: Option<ShippingProfileId>,
        limit: u32,
    ) -> Result<Vec<ProfileSummary>, ShippingError> {
        sqlx::query_as::<_, SummaryRow>(
            r#"
            SELECT shipping_profile_id,listing_source_id,name,management_revision,terms_revision,lifecycle FROM
            shipping_profiles WHERE listing_source_id=$1 AND ($2::uuid IS NULL OR shipping_profile_id>$2) ORDER
            BY shipping_profile_id LIMIT $3
        "#,
        )
        .bind(source.into_uuid())
        .bind(after.map(|p|p.into_uuid()))
        .bind(i64::from(limit))
        .fetch_all(&mut *self.0)
        .await
        .map_err(sql_error)?.into_iter().map(SummaryRow::view).collect()
    }
    async fn get_default(&mut self, source: ListingSourceId) -> Result<DefaultView, ShippingError> {
        let row: Option<(Option<Uuid>, i64, bool, Option<String>)> = sqlx::query_as(
            r#"
            SELECT d.shipping_profile_id,d.revision,d.source_deleted,p.lifecycle FROM shipping_source_defaults d
            LEFT JOIN shipping_profiles p ON p.shipping_profile_id=d.shipping_profile_id AND
            p.listing_source_id=d.listing_source_id WHERE d.listing_source_id=$1
        "#,
        )
        .bind(source.into_uuid())
        .fetch_optional(&mut *self.0)
        .await
        .map_err(sql_error)?;
        let Some((id, revision, deleted, lifecycle)) = row else {
            return Ok(DefaultView {
                source_id: source,
                revision: ShippingDefaultRevision::INITIAL,
                profile_id: None,
                eligibility: None,
            });
        };
        let d = DefaultRow {
            listing_source_id: source.into_uuid(),
            shipping_profile_id: id,
            revision,
            source_deleted: deleted,
        }
        .stored()?;
        let eligibility = match (id, lifecycle) {
            (None, None) => None,
            (Some(_), None) => Some(DefaultEligibility::Missing),
            (Some(_), Some(v)) => Some(
                match ShippingProfileLifecycle::from_code(&v).ok_or_else(corrupt)? {
                    ShippingProfileLifecycle::Active => DefaultEligibility::Active,
                    ShippingProfileLifecycle::Retired => DefaultEligibility::Retired,
                },
            ),
            _ => return Err(corrupt()),
        };
        Ok(DefaultView {
            source_id: source,
            revision: d.revision,
            profile_id: d.selection.profile_id(),
            eligibility,
        })
    }
}

pub mod projection;

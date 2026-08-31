use party_core::{
    party::{Party, PartyContact, RehydratedPartyState},
    party_id::PartyId,
    party_name::PartyName,
};
use party_service::ports::{PartyStorageVersion, StoredParty};
use serde_email::Email;
use time::OffsetDateTime;

#[derive(Debug, sqlx::FromRow)]
pub(crate) struct PartyRow {
    party_id: uuid::Uuid,
    party_slug_id: String,
    name: String,
    phone: Option<String>,
    email: Option<String>,
    version: i64,
    created: OffsetDateTime,
    updated: OffsetDateTime,
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum PartyRowMappingError {
    #[error("invalid party name persisted")]
    Name,
    #[error("invalid party slug persisted")]
    Slug,
    #[error("invalid party email persisted")]
    Email,
    #[error("invalid party version persisted")]
    Version,
}

const PARTY_COLUMNS: &str = r#"
    party_id, party_slug_id, name, phone, email, version, created, updated
"#;

pub(crate) fn party_columns() -> &'static str {
    PARTY_COLUMNS
}

pub(crate) fn version_to_i64(version: PartyStorageVersion) -> Result<i64, PartyRowMappingError> {
    i64::try_from(version.into_inner()).map_err(|_| PartyRowMappingError::Version)
}

impl TryFrom<PartyRow> for StoredParty {
    type Error = PartyRowMappingError;

    fn try_from(row: PartyRow) -> Result<Self, Self::Error> {
        let version = PartyStorageVersion::try_from(row.version)
            .map_err(|_| PartyRowMappingError::Version)?;
        let party = Party::rehydrate(RehydratedPartyState {
            id: PartyId::from(row.party_id),
            slug_id: row.party_slug_id,
            name: PartyName::try_from(row.name).map_err(|_| PartyRowMappingError::Name)?,
            contact: PartyContact {
                phone: row.phone,
                email: row
                    .email
                    .as_deref()
                    .map(Email::try_from)
                    .transpose()
                    .map_err(|_| PartyRowMappingError::Email)?,
            },
        })
        .map_err(|_| PartyRowMappingError::Slug)?;

        Ok(StoredParty {
            party,
            version,
            created: row.created,
            updated: row.updated,
        })
    }
}

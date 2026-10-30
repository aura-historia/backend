use crate::ports::{PartyRepository, PartyRepositoryError, StoredParty};
use party_core::party::{NewParty, Party};

/// Persists a new Party in the caller's transaction.
pub async fn insert_new_party(
    parties: &mut impl PartyRepository,
    input: NewParty,
) -> Result<StoredParty, PartyRepositoryError> {
    parties.insert(&Party::create(input)).await
}

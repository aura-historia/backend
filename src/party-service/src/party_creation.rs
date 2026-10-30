use crate::ports::{PartyRepository, PartyRepositoryError, StoredParty};
use application::error::static_error;
use party_core::{
    party::{NewParty, Party},
    party_slug_id::PartySlugId,
};

/// Bounds the inserts one creation may spend on slug candidates of a heavily reused name.
const MAX_PARTY_SLUG_CANDIDATES: usize = 100;

/// Persists a new Party in the caller's transaction under the first of its
/// [`PartySlugId::candidates`] that no other Party holds.
pub async fn insert_new_party(
    parties: &mut impl PartyRepository,
    input: NewParty,
) -> Result<StoredParty, PartyRepositoryError> {
    for slug_id in PartySlugId::candidates(&input.name).take(MAX_PARTY_SLUG_CANDIDATES) {
        match parties.insert(&Party::create(input.clone(), slug_id)).await {
            Err(PartyRepositoryError::SlugConflict { .. }) => {}
            inserted => return inserted,
        }
    }
    Err(PartyRepositoryError::SlugConflict {
        source: static_error("every party slug candidate is taken"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ports::{PartyDeletionBlocker, PartyStorageVersion};
    use party_core::{party::PartyContact, party_id::PartyId, party_name::PartyName};
    use std::collections::HashSet;
    use time::OffsetDateTime;

    #[derive(Default)]
    struct FakeParties {
        taken: HashSet<String>,
        insert_error: Option<PartyRepositoryError>,
        attempted: Vec<String>,
    }

    fn unexpected() -> PartyRepositoryError {
        PartyRepositoryError::Internal {
            source: static_error("unexpected party repository call"),
        }
    }

    #[async_trait::async_trait]
    impl PartyRepository for FakeParties {
        async fn find_by_id(
            &mut self,
            _: PartyId,
        ) -> Result<Option<StoredParty>, PartyRepositoryError> {
            Err(unexpected())
        }

        async fn find_by_slug(
            &mut self,
            _: &PartySlugId,
        ) -> Result<Option<StoredParty>, PartyRepositoryError> {
            Err(unexpected())
        }

        async fn find_by_id_for_update(
            &mut self,
            _: PartyId,
        ) -> Result<Option<StoredParty>, PartyRepositoryError> {
            Err(unexpected())
        }

        async fn find_deletion_blocker(
            &mut self,
            _: PartyId,
        ) -> Result<Option<PartyDeletionBlocker>, PartyRepositoryError> {
            Err(unexpected())
        }

        async fn delete_unused(
            &mut self,
            _: PartyId,
            _: PartyStorageVersion,
        ) -> Result<(), PartyRepositoryError> {
            Err(unexpected())
        }

        async fn insert(&mut self, party: &Party) -> Result<StoredParty, PartyRepositoryError> {
            let slug_id = party.slug_id().to_string();
            self.attempted.push(slug_id.clone());
            if let Some(error) = self.insert_error.take() {
                return Err(error);
            }
            if !self.taken.insert(slug_id) {
                return Err(PartyRepositoryError::SlugConflict {
                    source: static_error("party slug already exists"),
                });
            }
            Ok(StoredParty {
                party: party.clone(),
                version: PartyStorageVersion::INITIAL,
                created: OffsetDateTime::UNIX_EPOCH,
                updated: OffsetDateTime::UNIX_EPOCH,
            })
        }

        async fn update(
            &mut self,
            _: &Party,
            _: PartyStorageVersion,
        ) -> Result<StoredParty, PartyRepositoryError> {
            Err(unexpected())
        }
    }

    fn new_party(name: &str) -> NewParty {
        NewParty {
            id: PartyId::new(),
            name: PartyName::try_from(name)
                .unwrap_or_else(|error| panic!("invalid test party name: {error}")),
            contact: PartyContact::default(),
        }
    }

    fn taken(slugs: &[&str]) -> HashSet<String> {
        slugs.iter().map(|slug| (*slug).to_owned()).collect()
    }

    #[tokio::test]
    async fn should_insert_party_under_slugified_name_when_it_is_free() {
        let mut parties = FakeParties::default();

        let stored = insert_new_party(&mut parties, new_party("Antik und Stil")).await;

        assert!(matches!(
            stored,
            Ok(StoredParty { ref party, .. }) if party.slug_id().as_ref() == "antik-und-stil"
        ));
        assert_eq!(vec!["antik-und-stil"], parties.attempted);
    }

    #[tokio::test]
    async fn should_insert_party_under_first_free_numbered_slug_when_earlier_ones_are_taken() {
        let mut parties = FakeParties {
            taken: taken(&["antik-und-stil", "antik-und-stil-2"]),
            ..FakeParties::default()
        };

        let stored = insert_new_party(&mut parties, new_party("Antik und Stil")).await;

        assert!(matches!(
            stored,
            Ok(StoredParty { ref party, .. }) if party.slug_id().as_ref() == "antik-und-stil-3"
        ));
        assert_eq!(
            vec!["antik-und-stil", "antik-und-stil-2", "antik-und-stil-3"],
            parties.attempted
        );
    }

    #[tokio::test]
    async fn should_report_slug_conflict_when_every_candidate_is_taken() {
        let input = new_party("Antik und Stil");
        let mut parties = FakeParties {
            taken: PartySlugId::candidates(&input.name)
                .take(MAX_PARTY_SLUG_CANDIDATES)
                .map(|slug_id| slug_id.to_string())
                .collect(),
            ..FakeParties::default()
        };

        let stored = insert_new_party(&mut parties, input).await;

        assert!(matches!(
            stored,
            Err(PartyRepositoryError::SlugConflict { .. })
        ));
        assert_eq!(MAX_PARTY_SLUG_CANDIDATES, parties.attempted.len());
    }

    #[tokio::test]
    async fn should_not_try_further_slugs_after_other_insert_failure() {
        let mut parties = FakeParties {
            insert_error: Some(PartyRepositoryError::TemporarilyUnavailable {
                source: static_error("database unavailable"),
            }),
            ..FakeParties::default()
        };

        let stored = insert_new_party(&mut parties, new_party("Antik und Stil")).await;

        assert!(matches!(
            stored,
            Err(PartyRepositoryError::TemporarilyUnavailable { .. })
        ));
        assert_eq!(vec!["antik-und-stil"], parties.attempted);
    }
}

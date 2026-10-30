use crate::{
    party_id::PartyId,
    party_name::PartyName,
    party_slug_id::{InvalidPartySlugId, PartySlugId},
};
use domain_primitives::change_outcome::ChangeOutcome;
use serde_email::Email;

#[derive(Debug, Clone, PartialEq)]
pub struct Party {
    id: PartyId,
    slug_id: PartySlugId,
    name: PartyName,
    contact: PartyContact,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct PartyContact {
    pub phone: Option<String>,
    pub email: Option<Email>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct NewParty {
    pub id: PartyId,
    pub name: PartyName,
    pub contact: PartyContact,
}

#[doc(hidden)]
#[derive(Debug, Clone, PartialEq)]
pub struct RehydratedPartyState {
    pub id: PartyId,
    pub slug_id: String,
    pub name: PartyName,
    pub contact: PartyContact,
}

#[derive(Debug, thiserror::Error)]
pub enum RehydratePartyError {
    #[error("invalid persisted party slug")]
    InvalidSlug(#[source] InvalidPartySlugId),
}

impl Party {
    /// Creates a Party under `slug_id`, which callers pick from [`PartySlugId::candidates`] for
    /// its name. The slug never changes afterwards, not even when the Party is renamed.
    pub fn create(input: NewParty, slug_id: PartySlugId) -> Self {
        Self {
            id: input.id,
            slug_id,
            name: input.name,
            contact: input.contact,
        }
    }

    #[doc(hidden)]
    pub fn rehydrate(state: RehydratedPartyState) -> Result<Self, RehydratePartyError> {
        Ok(Self {
            id: state.id,
            slug_id: PartySlugId::raw(state.slug_id).map_err(RehydratePartyError::InvalidSlug)?,
            name: state.name,
            contact: state.contact,
        })
    }

    pub fn rename(&mut self, name: PartyName) -> ChangeOutcome {
        replace_if_changed(&mut self.name, name)
    }

    pub fn replace_contact(&mut self, contact: PartyContact) -> ChangeOutcome {
        replace_if_changed(&mut self.contact, contact)
    }

    pub fn id(&self) -> PartyId {
        self.id
    }

    pub fn slug_id(&self) -> &PartySlugId {
        &self.slug_id
    }

    pub fn name(&self) -> &PartyName {
        &self.name
    }

    pub fn contact(&self) -> &PartyContact {
        &self.contact
    }
}

fn replace_if_changed<T: PartialEq>(current: &mut T, replacement: T) -> ChangeOutcome {
    if *current == replacement {
        ChangeOutcome::Unchanged
    } else {
        *current = replacement;
        ChangeOutcome::Changed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn party_name(value: &str) -> PartyName {
        PartyName::try_from(value)
            .unwrap_or_else(|error| panic!("invalid test party name: {error}"))
    }

    #[test]
    fn should_preserve_slug_when_renaming_party() {
        let name = party_name("Antik und Stil");
        let slug_id = PartySlugId::candidates(&name)
            .next()
            .unwrap_or_else(|| panic!("party name has no slug candidate"));
        let mut party = Party::create(
            NewParty {
                id: PartyId::new(),
                name,
                contact: PartyContact::default(),
            },
            slug_id,
        );

        assert!(party.rename(party_name("Neue Identität")).changed());

        assert_eq!("antik-und-stil", party.slug_id().as_ref());
        assert_eq!("Neue Identität", party.name().as_ref());
    }

    #[test]
    fn should_rehydrate_exact_valid_persisted_slug_without_comparing_name() {
        let party = Party::rehydrate(RehydratedPartyState {
            id: PartyId::new(),
            slug_id: "historic-party-slug".to_owned(),
            name: party_name("Renamed Party"),
            contact: PartyContact::default(),
        });

        let party = party.unwrap_or_else(|error| panic!("failed to rehydrate party: {error}"));

        assert_eq!("historic-party-slug", party.slug_id().as_ref());
    }

    #[test]
    fn should_reject_invalid_persisted_slug_when_rehydrating_party() {
        let party = Party::rehydrate(RehydratedPartyState {
            id: PartyId::new(),
            slug_id: "historic--party".to_owned(),
            name: party_name("Party"),
            contact: PartyContact::default(),
        });

        assert!(matches!(party, Err(RehydratePartyError::InvalidSlug(_))));
    }
}

use crate::{party_id::PartyId, party_name::PartyName, party_slug_id::PartySlugId};
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
    pub slug_id: PartySlugId,
    pub name: PartyName,
    pub contact: PartyContact,
}

impl Party {
    pub fn create(input: NewParty) -> Self {
        let slug_id = PartySlugId::from(input.name.as_ref());
        Self {
            id: input.id,
            slug_id,
            name: input.name,
            contact: input.contact,
        }
    }

    #[doc(hidden)]
    pub fn rehydrate(state: RehydratedPartyState) -> Self {
        Self {
            id: state.id,
            slug_id: state.slug_id,
            name: state.name,
            contact: state.contact,
        }
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

    #[test]
    fn should_derive_slug_once_when_creating_party() {
        let party = Party::create(NewParty {
            id: PartyId::new(),
            name: PartyName::from("Antik und Stil"),
            contact: PartyContact::default(),
        });

        assert_eq!("antik-und-stil", party.slug_id().as_ref());
    }

    #[test]
    fn should_preserve_slug_when_renaming_party() {
        let mut party = Party::create(NewParty {
            id: PartyId::new(),
            name: PartyName::from("Antik und Stil"),
            contact: PartyContact::default(),
        });

        assert!(party.rename(PartyName::from("Neue Identität")).changed());

        assert_eq!("antik-und-stil", party.slug_id().as_ref());
        assert_eq!("Neue Identität", party.name().as_ref());
    }

    #[test]
    fn should_rehydrate_valid_persisted_slug_without_comparing_name() {
        let party = Party::rehydrate(RehydratedPartyState {
            id: PartyId::new(),
            slug_id: PartySlugId::raw("historic-party-slug").unwrap_or_else(|error| {
                panic!("invalid test slug: {error}");
            }),
            name: PartyName::from("Renamed Party"),
            contact: PartyContact::default(),
        });

        assert_eq!("historic-party-slug", party.slug_id().as_ref());
    }
}

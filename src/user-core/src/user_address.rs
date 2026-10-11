use crate::user_id::UserId;
use geo::{CountryCode, GeographicDescription, PostalCode};

domain_primitives::object_id_newtype!(UserAddressId, "uaddr");

#[derive(Clone, PartialEq, Eq)]
pub struct UserAddressLabel(String);

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("invalid user address")]
pub struct InvalidUserAddress;

impl UserAddressLabel {
    pub fn new(value: String) -> Result<Self, InvalidUserAddress> {
        if value.trim().is_empty() || value.len() > 128 || value.chars().any(char::is_control) {
            return Err(InvalidUserAddress);
        }
        Ok(Self(value))
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}
impl std::fmt::Debug for UserAddressLabel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("UserAddressLabel(<redacted>)")
    }
}

/// An editable private bookmark. Its owner and identity never change.
#[derive(Clone, PartialEq, Eq)]
pub struct UserAddress {
    id: UserAddressId,
    owner: UserId,
    label: UserAddressLabel,
    geography: GeographicDescription,
    input_revision: u64,
}
impl UserAddress {
    pub fn create(
        id: UserAddressId,
        owner: UserId,
        label: UserAddressLabel,
        geography: GeographicDescription,
    ) -> Result<Self, InvalidUserAddress> {
        Self::rehydrate(id, owner, label, geography, 1)
    }
    pub fn rehydrate(
        id: UserAddressId,
        owner: UserId,
        label: UserAddressLabel,
        geography: GeographicDescription,
        input_revision: u64,
    ) -> Result<Self, InvalidUserAddress> {
        if geography.address_text().is_none() || !(1..=i64::MAX as u64).contains(&input_revision) {
            return Err(InvalidUserAddress);
        }
        Ok(Self {
            id,
            owner,
            label,
            geography,
            input_revision,
        })
    }
    pub fn replace(
        &mut self,
        label: UserAddressLabel,
        geography: GeographicDescription,
    ) -> Result<bool, InvalidUserAddress> {
        if geography.address_text().is_none() {
            return Err(InvalidUserAddress);
        }
        let changed = self.label != label || self.geography != geography;
        if self.geography != geography {
            self.input_revision = self
                .input_revision
                .checked_add(1)
                .filter(|v| *v <= i64::MAX as u64)
                .ok_or(InvalidUserAddress)?;
        }
        self.label = label;
        self.geography = geography;
        Ok(changed)
    }
    pub fn id(&self) -> UserAddressId {
        self.id
    }
    pub fn owner(&self) -> UserId {
        self.owner
    }
    pub fn label(&self) -> &UserAddressLabel {
        &self.label
    }
    pub fn geography(&self) -> &GeographicDescription {
        &self.geography
    }
    pub fn input_revision(&self) -> u64 {
        self.input_revision
    }
}
impl std::fmt::Debug for UserAddress {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("UserAddress")
            .field("id", &self.id)
            .finish_non_exhaustive()
    }
}

/// A browsing choice, never an assertion of residence or deliverability.
#[derive(Clone, PartialEq, Eq, Default)]
pub enum BrowsingDestination {
    #[default]
    None,
    Address(UserAddressId),
    CountryPostal {
        country: CountryCode,
        postal_code: Option<PostalCode>,
    },
}
impl std::fmt::Debug for BrowsingDestination {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("BrowsingDestination(<redacted>)")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use geo::AddressText;
    fn description(text: &str) -> GeographicDescription {
        GeographicDescription::new(Some(AddressText::new(text).unwrap()), None, None, None)
            .unwrap()
            .unwrap()
    }
    #[test]
    fn rejects_invalid_labels_and_missing_text_without_losing_identity() {
        for label in ["".to_owned(), " ".into(), "a\nb".into(), "é".repeat(65)] {
            assert!(UserAddressLabel::new(label).is_err());
        }
        let id = UserAddressId::new();
        let owner = UserId::new();
        let label = UserAddressLabel::new("Home".into()).unwrap();
        assert!(
            UserAddress::create(
                id,
                owner,
                label.clone(),
                GeographicDescription::new(None, Some(CountryCode::GBR), None, None)
                    .unwrap()
                    .unwrap()
            )
            .is_err()
        );
        let mut address =
            UserAddress::create(id, owner, label, description("Synthetic one")).unwrap();
        address
            .replace(
                UserAddressLabel::new("Renamed".into()).unwrap(),
                description("Synthetic one"),
            )
            .unwrap();
        assert_eq!(1, address.input_revision());
        address
            .replace(
                UserAddressLabel::new("Renamed".into()).unwrap(),
                description("Synthetic two"),
            )
            .unwrap();
        assert_eq!(2, address.input_revision());
        assert_eq!(id, address.id());
        assert_eq!(owner, address.owner());
        assert!(!format!("{address:?}").contains("Synthetic"));
    }
    #[test]
    fn strict_identity_and_revision_exhaustion_fail_closed() {
        let id = UserAddressId::new();
        assert!(id.to_string().starts_with("uaddr_"));
        assert!(UserAddressId::try_from(id.into_uuid().to_string()).is_err());
        assert!(UserAddressId::try_from(UserId::new().to_string()).is_err());
        let label = UserAddressLabel::new("Home".into()).unwrap();
        assert!(
            UserAddress::rehydrate(
                id,
                UserId::new(),
                label.clone(),
                description("Synthetic"),
                0
            )
            .is_err()
        );
        let mut address = UserAddress::rehydrate(
            id,
            UserId::new(),
            label.clone(),
            description("Synthetic"),
            i64::MAX as u64,
        )
        .unwrap();
        let before = address.clone();
        assert!(address.replace(label, description("Replacement")).is_err());
        assert_eq!(before, address);
    }
}

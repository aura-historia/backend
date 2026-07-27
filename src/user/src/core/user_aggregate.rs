use crate::core::{
    first_name::FirstName, last_name::LastName, name::Name, role::UserRole, tier::UserTier,
};
use common::change_outcome::ChangeOutcome;
use common::{
    currency::domain::Currency, language::domain::Language,
    measurement_unit::domain::MeasurementUnit, shop_id::ShopId,
    stripe_customer_id::StripeCustomerId, user_id::UserId,
};
use geo::core::address::{GeoAddress, StructuredAddress};
use serde_email::Email;
use std::collections::HashSet;

#[derive(Debug, Clone, PartialEq)]
pub struct User {
    id: UserId,
    email: Email,
    profile: UserProfile,
    preferences: UserPreferences,
    account: UserAccount,
    partner_shops: HashSet<ShopId>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct NewUser {
    pub id: UserId,
    pub email: Email,
    pub profile: UserProfile,
    pub preferences: UserPreferences,
    pub account: UserAccount,
    pub partner_shops: HashSet<ShopId>,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct UserState {
    pub id: UserId,
    pub email: Email,
    pub profile: UserProfile,
    pub preferences: UserPreferences,
    pub account: UserAccount,
    pub partner_shops: HashSet<ShopId>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct UserProfile {
    pub first_name: Option<FirstName>,
    pub last_name: Option<LastName>,
    pub structured_address: Option<StructuredAddress>,
    pub geo_address: Option<GeoAddress>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct UserPreferences {
    pub language: Option<Language>,
    pub currency: Option<Currency>,
    pub measurement_unit: Option<MeasurementUnit>,
    pub prohibited_content_consent: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct UserAccount {
    pub tier: UserTier,
    pub role: UserRole,
    pub stripe_customer_id: Option<StripeCustomerId>,
}

impl Default for UserAccount {
    fn default() -> Self {
        Self {
            tier: UserTier::Free,
            role: UserRole::User,
            stripe_customer_id: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum RehydrateUserError {
    #[error("user geo latitude out of range")]
    GeoLatitudeOutOfRange,
    #[error("user geo longitude out of range")]
    GeoLongitudeOutOfRange,
}

impl User {
    pub fn create(input: NewUser) -> Result<Self, RehydrateUserError> {
        Self::rehydrate(UserState {
            id: input.id,
            email: input.email,
            profile: input.profile,
            preferences: input.preferences,
            account: input.account,
            partner_shops: input.partner_shops,
        })
    }

    #[allow(dead_code)]
    pub(crate) fn rehydrate(state: UserState) -> Result<Self, RehydrateUserError> {
        validate_geo_address(state.profile.geo_address)?;

        Ok(Self {
            id: state.id,
            email: state.email,
            profile: state.profile,
            preferences: state.preferences,
            account: state.account,
            partner_shops: state.partner_shops,
        })
    }

    pub fn change_email(&mut self, email: Email) -> ChangeOutcome {
        replace_if_changed(&mut self.email, email)
    }

    pub fn replace_profile(
        &mut self,
        profile: UserProfile,
    ) -> Result<ChangeOutcome, RehydrateUserError> {
        validate_geo_address(profile.geo_address)?;
        Ok(replace_if_changed(&mut self.profile, profile))
    }

    pub fn replace_preferences(&mut self, preferences: UserPreferences) -> ChangeOutcome {
        replace_if_changed(&mut self.preferences, preferences)
    }

    pub fn change_tier(&mut self, tier: UserTier) -> ChangeOutcome {
        replace_if_changed(&mut self.account.tier, tier)
    }

    pub fn change_role(&mut self, role: UserRole) -> ChangeOutcome {
        replace_if_changed(&mut self.account.role, role)
    }

    pub fn change_stripe_customer_id(
        &mut self,
        stripe_customer_id: Option<StripeCustomerId>,
    ) -> ChangeOutcome {
        replace_if_changed(&mut self.account.stripe_customer_id, stripe_customer_id)
    }

    pub fn grant_partner_shop(&mut self, shop_id: ShopId) -> ChangeOutcome {
        ChangeOutcome::from(self.partner_shops.insert(shop_id))
    }

    pub fn revoke_partner_shop(&mut self, shop_id: &ShopId) -> ChangeOutcome {
        ChangeOutcome::from(self.partner_shops.remove(shop_id))
    }

    pub fn id(&self) -> UserId {
        self.id
    }

    pub fn email(&self) -> &Email {
        &self.email
    }

    pub fn profile(&self) -> &UserProfile {
        &self.profile
    }

    pub fn preferences(&self) -> &UserPreferences {
        &self.preferences
    }

    pub fn account(&self) -> &UserAccount {
        &self.account
    }

    pub fn partner_shops(&self) -> &HashSet<ShopId> {
        &self.partner_shops
    }

    pub fn name(&self) -> Option<Name> {
        match (
            self.profile.first_name.as_ref(),
            self.profile.last_name.as_ref(),
        ) {
            (Some(first), Some(last)) => Some(Name::from(format!("{first} {last}"))),
            (Some(first), None) => Some(Name::from(first.as_ref())),
            (None, Some(last)) => Some(Name::from(last.as_ref())),
            (None, None) => None,
        }
    }
}

fn validate_geo_address(geo_address: Option<GeoAddress>) -> Result<(), RehydrateUserError> {
    if let Some(address) = geo_address {
        if !(-90.0..=90.0).contains(&address.lat) {
            return Err(RehydrateUserError::GeoLatitudeOutOfRange);
        }
        if !(-180.0..=180.0).contains(&address.lon) {
            return Err(RehydrateUserError::GeoLongitudeOutOfRange);
        }
    }

    Ok(())
}

fn replace_if_changed<T: PartialEq>(target: &mut T, value: T) -> ChangeOutcome {
    if *target == value {
        ChangeOutcome::Unchanged
    } else {
        *target = value;
        ChangeOutcome::Changed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn email(value: &str) -> Email {
        match Email::try_from(value) {
            Ok(email) => email,
            Err(error) => panic!("invalid test email: {error}"),
        }
    }

    fn new_user() -> NewUser {
        NewUser {
            id: UserId::new(),
            email: email("ada@example.com"),
            profile: UserProfile {
                first_name: Some("Ada".into()),
                last_name: Some("Lovelace".into()),
                structured_address: None,
                geo_address: None,
            },
            preferences: UserPreferences::default(),
            account: UserAccount::default(),
            partner_shops: HashSet::new(),
        }
    }

    fn user_state_with_geo(lat: f64, lon: f64) -> UserState {
        let input = new_user();
        UserState {
            id: input.id,
            email: input.email,
            profile: UserProfile {
                geo_address: Some(GeoAddress { lat, lon }),
                ..input.profile
            },
            preferences: input.preferences,
            account: input.account,
            partner_shops: input.partner_shops,
        }
    }

    #[test]
    fn should_create_user_with_private_state_and_defaults() {
        let result = User::create(new_user());

        assert!(matches!(result, Ok(ref user) if user.account().tier == UserTier::Free));
    }

    #[test]
    fn should_return_user_name_when_profile_has_names() {
        let result = User::create(new_user());

        assert!(matches!(
            result.and_then(|user| user.name().ok_or(RehydrateUserError::GeoLatitudeOutOfRange)),
            Ok(ref name) if name.as_ref() == "Ada Lovelace"
        ));
    }

    #[test]
    fn should_change_tier_when_tier_differs() {
        let mut user = match User::create(new_user()) {
            Ok(user) => user,
            Err(error) => panic!("user create failed: {error}"),
        };

        let outcome = user.change_tier(UserTier::Pro);

        assert_eq!(ChangeOutcome::Changed, outcome);
        assert_eq!(UserTier::Pro, user.account().tier);
    }

    #[test]
    fn should_report_unchanged_when_tier_same() {
        let mut user = match User::create(new_user()) {
            Ok(user) => user,
            Err(error) => panic!("user create failed: {error}"),
        };

        let outcome = user.change_tier(UserTier::Free);

        assert_eq!(ChangeOutcome::Unchanged, outcome);
    }

    #[test]
    fn should_grant_partner_shop_when_not_present() {
        let mut user = match User::create(new_user()) {
            Ok(user) => user,
            Err(error) => panic!("user create failed: {error}"),
        };
        let shop_id = ShopId::new();

        let outcome = user.grant_partner_shop(shop_id);

        assert_eq!(ChangeOutcome::Changed, outcome);
        assert!(user.partner_shops().contains(&shop_id));
    }

    #[test]
    fn should_report_unchanged_when_partner_shop_already_granted() {
        let mut user = match User::create(new_user()) {
            Ok(user) => user,
            Err(error) => panic!("user create failed: {error}"),
        };
        let shop_id = ShopId::new();
        let _ = user.grant_partner_shop(shop_id);

        let outcome = user.grant_partner_shop(shop_id);

        assert_eq!(ChangeOutcome::Unchanged, outcome);
    }

    #[test]
    fn should_reject_rehydrate_when_latitude_invalid() {
        let result = User::rehydrate(user_state_with_geo(91.0, 8.0));

        assert!(matches!(
            result,
            Err(RehydrateUserError::GeoLatitudeOutOfRange)
        ));
    }

    #[test]
    fn should_reject_rehydrate_when_longitude_invalid() {
        let result = User::rehydrate(user_state_with_geo(52.0, 181.0));

        assert!(matches!(
            result,
            Err(RehydrateUserError::GeoLongitudeOutOfRange)
        ));
    }
}

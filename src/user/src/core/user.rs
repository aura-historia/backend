use crate::core::{
    first_name::FirstName, last_name::LastName, name::Name, role::UserRole, tier::UserTier,
};
use common::{
    actor::domain::Actor, currency::domain::Currency, language::domain::Language,
    measurement_unit::domain::MeasurementUnit, shop_id::ShopId,
    stripe_customer_id::StripeCustomerId, user_id::UserId,
};
use geo::core::address::{GeoAddress, StructuredAddress};
use serde_email::Email;
use std::collections::HashSet;
use time::OffsetDateTime;

#[derive(Debug, Clone, PartialEq)]
pub struct User {
    pub user_id: UserId,
    pub email: Email,
    pub first_name: Option<FirstName>,
    pub last_name: Option<LastName>,
    pub language: Option<Language>,
    pub currency: Option<Currency>,
    pub measurement_unit: Option<MeasurementUnit>,
    pub prohibited_content_consent: bool,
    pub tier: UserTier,
    pub role: UserRole,
    pub stripe_customer_id: Option<StripeCustomerId>,
    pub structured_address: Option<StructuredAddress>,
    pub geo_address: Option<GeoAddress>,
    pub partner_shops: HashSet<ShopId>,
    pub created_by: Actor,
    pub updated_by: Actor,
    pub created: OffsetDateTime,
    pub updated: OffsetDateTime,
}

impl User {
    /// Returns the user's full display name composed from the available
    /// `first_name` / `last_name` parts, or `None` if neither is set.
    pub fn name(&self) -> Option<Name> {
        match (self.first_name.as_ref(), self.last_name.as_ref()) {
            (Some(first), Some(last)) => Some(Name::from(format!("{first} {last}"))),
            (Some(first), None) => Some(Name::from(first.as_ref())),
            (None, Some(last)) => Some(Name::from(last.as_ref())),
            (None, None) => None,
        }
    }
}

#[cfg(feature = "test-data")]
mod fake {
    use crate::core::user::User;
    use fake::{Fake, faker::internet::en::DomainSuffix};
    use time::OffsetDateTime;

    impl fake::Dummy<fake::Faker> for User {
        fn dummy_with_rng<R: fake::rand::RngExt + ?Sized>(
            config: &fake::Faker,
            rng: &mut R,
        ) -> Self {
            let domain_str: String = DomainSuffix().fake_with_rng(rng);
            let first_name = config.fake_with_rng(rng);
            let last_name = config.fake_with_rng(rng);
            User {
                user_id: config.fake_with_rng(rng),
                email: format!("{first_name}.{last_name}@{domain_str}")
                    .try_into()
                    .unwrap(),
                first_name: Some(first_name),
                last_name: Some(last_name),
                language: config.fake_with_rng(rng),
                currency: config.fake_with_rng(rng),
                measurement_unit: config.fake_with_rng(rng),
                prohibited_content_consent: config.fake_with_rng(rng),
                tier: crate::core::tier::UserTier::Ultimate,
                role: config.fake_with_rng(rng),
                stripe_customer_id: config.fake_with_rng(rng),
                structured_address: config.fake_with_rng(rng),
                geo_address: config.fake_with_rng(rng),
                partner_shops: config.fake_with_rng(rng),
                created_by: config.fake_with_rng(rng),
                updated_by: config.fake_with_rng(rng),
                created: OffsetDateTime::now_utc(),
                updated: OffsetDateTime::now_utc(),
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use crate::core::user::User;
        use fake::{Fake, Faker};

        #[test]
        fn should_fake_user() {
            let _ = Faker.fake::<User>();
        }
    }
}

#[cfg(all(test, feature = "test-data"))]
mod name_tests {
    use crate::core::user::User;
    use fake::{Fake, Faker};

    #[test]
    fn should_return_first_and_last_name_when_both_set_for_full_name() {
        let mut user: User = Faker.fake();
        user.first_name = Some("Ada".into());
        user.last_name = Some("Lovelace".into());
        assert_eq!(user.name().unwrap().as_ref(), "Ada Lovelace");
    }

    #[test]
    fn should_return_first_name_when_only_first_set_for_full_name() {
        let mut user: User = Faker.fake();
        user.first_name = Some("Ada".into());
        user.last_name = None;
        assert_eq!(user.name().unwrap().as_ref(), "Ada");
    }

    #[test]
    fn should_return_last_name_when_only_last_set_for_full_name() {
        let mut user: User = Faker.fake();
        user.first_name = None;
        user.last_name = Some("Lovelace".into());
        assert_eq!(user.name().unwrap().as_ref(), "Lovelace");
    }

    #[test]
    fn should_return_none_when_neither_first_nor_last_set_for_full_name() {
        let mut user: User = Faker.fake();
        user.first_name = None;
        user.last_name = None;
        assert!(user.name().is_none());
    }
}

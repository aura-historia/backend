use crate::core::{first_name::FirstName, last_name::LastName};
use common::{currency::domain::Currency, language::domain::Language, user_id::UserId};
use serde_email::Email;
use time::OffsetDateTime;

#[derive(Debug, Clone, PartialEq)]
pub struct User {
    pub user_id: UserId,
    pub email: Email,
    pub first_name: Option<FirstName>,
    pub last_name: Option<LastName>,
    pub language: Option<Language>,
    pub currency: Option<Currency>,
    pub created: OffsetDateTime,
    pub updated: OffsetDateTime,
}

#[cfg(feature = "test-data")]
mod fake {
    use crate::core::user::User;
    use fake::{Fake, Faker, faker::internet::en::DomainSuffix};
    use time::OffsetDateTime;

    impl fake::Dummy<fake::Faker> for User {
        fn dummy_with_rng<R: fake::rand::Rng + ?Sized>(config: &fake::Faker, rng: &mut R) -> Self {
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
                language: Faker.fake(),
                currency: Faker.fake(),
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

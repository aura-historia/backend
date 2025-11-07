use common::user_id::UserId;
use serde_email::Email;
use time::OffsetDateTime;

#[derive(Debug, Clone, PartialEq)]
pub struct User {
    pub id: UserId,
    pub email: Email,
    pub created: OffsetDateTime,
    pub updated: OffsetDateTime,
}

#[cfg(feature = "test-data")]
mod fake {
    use crate::core::user::User;
    use fake::{Fake, faker::internet::de_de::SafeEmail};
    use time::OffsetDateTime;

    impl fake::Dummy<fake::Faker> for User {
        fn dummy_with_rng<R: fake::rand::Rng + ?Sized>(config: &fake::Faker, rng: &mut R) -> Self {
            let email_str: String = SafeEmail().fake_with_rng(rng);
            User {
                id: config.fake_with_rng(rng),
                email: email_str.try_into().unwrap(),
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

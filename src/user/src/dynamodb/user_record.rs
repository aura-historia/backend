use crate::core::user::User;
use common::user_id::UserId;
use serde::{Deserialize, Serialize};
use serde_email::Email;
use time::OffsetDateTime;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UserRecord {
    pub pk: String,
    pub sk: String,
    pub id: UserId,
    pub email: Email,

    #[serde(with = "time::serde::rfc3339")]
    pub created: OffsetDateTime,

    #[serde(with = "time::serde::rfc3339")]
    pub updated: OffsetDateTime,
}

pub fn mk_pk(user_id: &UserId) -> String {
    format!("user#{user_id}")
}

pub fn mk_sk() -> &'static str {
    "user#details"
}

impl From<User> for UserRecord {
    fn from(user: User) -> Self {
        UserRecord {
            pk: mk_pk(&user.id),
            sk: mk_sk().to_owned(),
            id: user.id,
            email: user.email,
            created: user.created,
            updated: user.updated,
        }
    }
}

impl From<UserRecord> for User {
    fn from(record: UserRecord) -> Self {
        User {
            id: record.id,
            email: record.email,
            created: record.created,
            updated: record.updated,
        }
    }
}

#[cfg(feature = "test-data")]
mod fake {
    use crate::core::user::User;
    use crate::dynamodb::user_record::UserRecord;
    use fake::Fake;

    impl fake::Dummy<fake::Faker> for UserRecord {
        fn dummy_with_rng<R: fake::rand::Rng + ?Sized>(config: &fake::Faker, rng: &mut R) -> Self {
            config.fake_with_rng::<User, R>(rng).into()
        }
    }
}

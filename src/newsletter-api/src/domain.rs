use common::{currency::domain::Currency, language::domain::Language, user_id::UserId};
use serde_email::Email;
use user::core::{first_name::FirstName, last_name::LastName, tier::UserTier};

pub struct UpsertNewsletterSubscription {
    pub email: Email,
    pub first_name: Option<FirstName>,
    pub last_name: Option<LastName>,
    pub language: Option<Language>,
    pub currency: Option<Currency>,
    pub user_id: Option<UserId>,
    pub tier: Option<UserTier>,
}

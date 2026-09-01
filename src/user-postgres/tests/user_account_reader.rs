use ::application::transaction::{Transaction, UnitOfWork};
use ::platform_postgres::SqlxUnitOfWork;
use ::user_core::user_id::UserId;
use localization::Language;
use money::Currency;
use serde_email::Email;
use test_api::{IntegrationTestService, Postgres, aura_integration_test, get_postgres_client};
use user_core::first_name::FirstName;
use user_core::last_name::LastName;
use user_core::measurement_unit::MeasurementUnit;
use user_core::role::UserRole;
use user_core::tier::UserTier;
use user_core::user::{NewUser, User, UserAccount, UserPreferences, UserProfile};
use user_postgres::{SqlxUserAccountReaderFactory, SqlxUserRepositoryFactory};
use user_service::ports::{
    UserAccountReader, UserAccountReaderFactory, UserRepository, UserRepositoryFactory,
};

const BUSINESS_SCHEMA: Postgres = Postgres::new("migrations");

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_read_user_account_from_postgres() {
    let pool = get_postgres_client().await;
    let unit_of_work = SqlxUnitOfWork::new(pool);
    let users = SqlxUserRepositoryFactory::new();
    let accounts = SqlxUserAccountReaderFactory::new();
    let user = sample_user("postgres-account-reader", UserRole::User);

    let mut tx = begin(&unit_of_work).await;
    match users.in_transaction(&mut tx).insert(&user).await {
        Ok(_) => {}
        Err(error) => panic!("failed to insert user: {error:?}"),
    }

    let account_view = match accounts.in_transaction(&mut tx).find_by_id(user.id()).await {
        Ok(Some(view)) => view,
        Ok(None) => panic!("missing account view"),
        Err(error) => panic!("failed to read account view: {error:?}"),
    };
    commit(tx).await;

    assert_eq!(user.id(), account_view.user_id);
}

fn sample_user(slug: &str, role: UserRole) -> User {
    match User::create(NewUser {
        id: UserId::new(),
        email: email(&format!("{slug}@example.com")),
        profile: UserProfile {
            first_name: Some(FirstName::from("Ada")),
            last_name: Some(LastName::from("Lovelace")),
            geo_address: Some(geo::core::address::GeoAddress {
                lat: 51.5,
                lon: -0.1,
            }),
        },
        preferences: UserPreferences {
            language: Some(Language::En),
            currency: Some(Currency::Gbp),
            measurement_unit: Some(MeasurementUnit::Imperial),
            show_unassessed_or_sensitive_content: true,
        },
        account: UserAccount {
            tier: UserTier::Pro,
            role,
            stripe_customer_id: None,
        },
    }) {
        Ok(user) => user,
        Err(error) => panic!("failed to create user: {error}"),
    }
}

fn email(value: &str) -> Email {
    match Email::try_from(value) {
        Ok(email) => email,
        Err(error) => panic!("invalid test email: {error}"),
    }
}

async fn begin(unit_of_work: &SqlxUnitOfWork) -> ::platform_postgres::SqlxTransaction {
    match unit_of_work.begin().await {
        Ok(tx) => tx,
        Err(error) => panic!("failed to begin transaction: {error}"),
    }
}

async fn commit(tx: ::platform_postgres::SqlxTransaction) {
    if let Err(error) = tx.commit().await {
        panic!("failed to commit transaction: {error}");
    }
}

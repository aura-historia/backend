use ::application::transaction::{Transaction, UnitOfWork};
use ::common::currency::domain::Currency;
use ::common::language::domain::Language;
use ::common::measurement_unit::domain::MeasurementUnit;
use ::common::user_id::UserId;
use ::platform_postgres::SqlxUnitOfWork;
use geo::core::{address::StructuredAddress, continent::Continent};
use isocountry::CountryCode;
use serde_email::Email;
use test_api::{IntegrationTestService, Postgres, aura_integration_test, get_postgres_client};
use user_core::first_name::FirstName;
use user_core::last_name::LastName;
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
            structured_address: Some(StructuredAddress {
                addressline: Some("1 Test Street".to_owned()),
                addressline_extra: None,
                locality: Some("London".to_owned()),
                region: None,
                postal_code: Some("SW1A".to_owned()),
                country: Some(CountryCode::GBR),
                continent: Some(Continent::Europe),
            }),
            geo_address: Some(geo::core::address::GeoAddress {
                lat: 51.5,
                lon: -0.1,
            }),
        },
        preferences: UserPreferences {
            language: Some(Language::En),
            currency: Some(Currency::Gbp),
            measurement_unit: Some(MeasurementUnit::Imperial),
            prohibited_content_consent: true,
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

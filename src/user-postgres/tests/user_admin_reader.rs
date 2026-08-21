use ::application::transaction::{Transaction, UnitOfWork};
use ::platform_postgres::SqlxUnitOfWork;
use ::user_core::user_id::UserId;
use geo::core::{address::StructuredAddress, continent::Continent};
use isocountry::CountryCode;
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
use user_postgres::{SqlxUserAdminReaderFactory, SqlxUserRepositoryFactory};
use user_service::ports::{
    UserAdminReader, UserAdminReaderFactory, UserRepository, UserRepositoryFactory,
};

const BUSINESS_SCHEMA: Postgres = Postgres::new("migrations");

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_read_user_admin_view_from_postgres() {
    let pool = get_postgres_client().await;
    let unit_of_work = SqlxUnitOfWork::new(pool);
    let users = SqlxUserRepositoryFactory::new();
    let admins = SqlxUserAdminReaderFactory::new();
    let user = sample_user("postgres-admin-reader", UserRole::Admin);

    let mut tx = begin(&unit_of_work).await;
    match users.in_transaction(&mut tx).insert(&user).await {
        Ok(_) => {}
        Err(error) => panic!("failed to insert user: {error:?}"),
    }

    let admin_view = match admins
        .in_transaction(&mut tx)
        .find_admin_actor(user.id())
        .await
    {
        Ok(Some(view)) => view,
        Ok(None) => panic!("missing admin view"),
        Err(error) => panic!("failed to read admin view: {error:?}"),
    };
    commit(tx).await;

    assert_eq!(UserRole::Admin, admin_view.role);
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

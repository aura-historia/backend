use ::application::transaction::{Transaction, UnitOfWork};
use ::platform_postgres::SqlxUnitOfWork;
use ::user_core::stripe_customer_id::StripeCustomerId;
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
use user_postgres::{SqlxUserRepositoryFactory, SqlxUserStripeCustomerReaderFactory};
use user_service::ports::{
    UserRepository, UserRepositoryFactory, UserStripeCustomerReader,
    UserStripeCustomerReaderFactory,
};
use user_service::use_cases::queries::find_user_by_stripe_customer_id::FindUserByStripeCustomerIdRequest;

const BUSINESS_SCHEMA: Postgres = Postgres::new("migrations");

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_read_user_by_stripe_customer_id_from_postgres() {
    let pool = get_postgres_client().await;
    let unit_of_work = SqlxUnitOfWork::new(pool);
    let users = SqlxUserRepositoryFactory::new();
    let stripe = SqlxUserStripeCustomerReaderFactory::new();
    let user = sample_user(
        "postgres-stripe-reader",
        UserRole::User,
        Some("cus_postgres_stripe_reader"),
    );

    let mut tx = begin(&unit_of_work).await;
    match users.in_transaction(&mut tx).insert(&user).await {
        Ok(_) => {}
        Err(error) => panic!("failed to insert user: {error:?}"),
    }

    let stripe_view = match stripe
        .in_transaction(&mut tx)
        .find_by_stripe_customer_id(&FindUserByStripeCustomerIdRequest {
            stripe_customer_id: StripeCustomerId::from("cus_postgres_stripe_reader"),
        })
        .await
    {
        Ok(Some(view)) => view,
        Ok(None) => panic!("missing stripe view"),
        Err(error) => panic!("failed to read stripe view: {error:?}"),
    };
    commit(tx).await;

    assert_eq!(user.id(), stripe_view.user_id);
}

fn sample_user(slug: &str, role: UserRole, stripe_customer_id: Option<&str>) -> User {
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
            show_unassessed_or_sensitive_content: true,
        },
        account: UserAccount {
            tier: UserTier::Pro,
            role,
            stripe_customer_id: stripe_customer_id.map(StripeCustomerId::from),
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

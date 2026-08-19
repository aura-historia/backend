use ::application::transaction::{Transaction, UnitOfWork};
use ::common::currency::domain::Currency;
use ::common::language::domain::Language;
use ::common::measurement_unit::domain::MeasurementUnit;
use ::common::shop_id::ShopId;
use ::common::user_id::UserId;
use ::platform_postgres::SqlxUnitOfWork;
use geo::core::{address::StructuredAddress, continent::Continent};
use isocountry::CountryCode;
use serde_email::Email;
use shop_partner_postgres::SqlxUserPartnerShopsReaderFactory;
use shop_partner_service::ports::{UserPartnerShopsReader, UserPartnerShopsReaderFactory};
use shop_partner_service::use_cases::list_partner_shops::ListPartnerShopsRequest;
use test_api::{IntegrationTestService, Postgres, aura_integration_test, get_postgres_client};
use user_core::first_name::FirstName;
use user_core::last_name::LastName;
use user_core::role::UserRole;
use user_core::tier::UserTier;
use user_core::user::{NewUser, User, UserAccount, UserPreferences, UserProfile};
use user_postgres::SqlxUserRepositoryFactory;
use user_service::ports::{UserRepository, UserRepositoryFactory};

const BUSINESS_SCHEMA: Postgres = Postgres::new("migrations");

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_list_partner_shops_for_user_in_name_order() {
    let pool = get_postgres_client().await;
    let unit_of_work = SqlxUnitOfWork::new(pool.clone());
    let users = SqlxUserRepositoryFactory::new();
    let partner_shops = SqlxUserPartnerShopsReaderFactory::new();
    let user = sample_user("postgres-partner-user", UserRole::User);
    let b_shop = seed_shop(&pool, "z-partner-shop").await;
    let a_shop = seed_shop(&pool, "a-partner-shop").await;

    let mut tx = begin(&unit_of_work).await;
    match users.in_transaction(&mut tx).insert(&user).await {
        Ok(_) => {}
        Err(error) => panic!("failed to insert user: {error:?}"),
    }
    commit(tx).await;
    seed_partner_shop(&pool, user.id(), b_shop).await;
    seed_partner_shop(&pool, user.id(), a_shop).await;

    let mut tx = begin(&unit_of_work).await;
    let result = match partner_shops
        .in_transaction(&mut tx)
        .list_partner_shops(&ListPartnerShopsRequest { user_id: user.id() })
        .await
    {
        Ok(result) => result,
        Err(error) => panic!("failed to list partner shops: {error:?}"),
    };
    commit(tx).await;

    assert_eq!(
        vec![a_shop, b_shop],
        result
            .items
            .iter()
            .map(|item| item.shop_id)
            .collect::<Vec<_>>()
    );
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

async fn seed_shop(pool: &sqlx::PgPool, slug: &str) -> ShopId {
    let shop_id = ShopId::new();
    let result = sqlx::query(
        r#"
        INSERT INTO shops (shop_id, shop_slug_id, name, shop_type, partner_status, shop_domains)
        VALUES ($1, $2, $3, 'COMMERCIAL_DEALER', 'SCRAPED', $4)
        "#,
    )
    .bind(uuid::Uuid::from(shop_id))
    .bind(slug)
    .bind(slug)
    .bind(Vec::<String>::from([format!("{slug}.example")]))
    .execute(pool)
    .await;

    if let Err(error) = result {
        panic!("failed to seed shop: {error}");
    }

    shop_id
}

async fn seed_partner_shop(pool: &sqlx::PgPool, user_id: UserId, shop_id: ShopId) {
    let result = sqlx::query("INSERT INTO user_partner_shops (user_id, shop_id) VALUES ($1, $2)")
        .bind(uuid::Uuid::from(user_id))
        .bind(uuid::Uuid::from(shop_id))
        .execute(pool)
        .await;

    if let Err(error) = result {
        panic!("failed to seed partner shop: {error}");
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

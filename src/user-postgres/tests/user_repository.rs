use ::application::transaction::{Transaction, UnitOfWork};
use ::platform_postgres::SqlxUnitOfWork;
use ::user_core::stripe_customer_id::StripeCustomerId;
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
use user_core::user::{
    NewUser, RehydratedUserState, User, UserAccount, UserPreferences, UserProfile,
};
use user_postgres::{SqlxMarketingConsentIntentRepository, SqlxUserRepositoryFactory};
use user_service::ports::{
    ConsentIntentSource, MarketingConsentIntents, MarketingConsentIntentsFactory,
    UserInsertOutcome, UserRepository, UserRepositoryError, UserRepositoryFactory,
    UserStorageVersion,
};

const BUSINESS_SCHEMA: Postgres = Postgres::new("migrations");

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_insert_find_update_user_in_postgres() {
    let pool = get_postgres_client().await;
    let unit_of_work = SqlxUnitOfWork::new(pool);
    let users = SqlxUserRepositoryFactory::new();
    let mut user = sample_user("postgres-main", UserRole::Admin, Some("cus_postgres_main"));

    let mut tx = begin(&unit_of_work).await;
    match users.in_transaction(&mut tx).insert(&user).await {
        Ok(_) => {}
        Err(error) => panic!("failed to insert user: {error:?}"),
    }
    let loaded_by_id = match users.in_transaction(&mut tx).find_by_id(user.id()).await {
        Ok(Some(loaded)) => loaded,
        Ok(None) => panic!("missing user by id"),
        Err(error) => panic!("failed to find user by id: {error:?}"),
    };
    let loaded_by_email = match users
        .in_transaction(&mut tx)
        .find_by_email(user.email())
        .await
    {
        Ok(Some(loaded)) => loaded,
        Ok(None) => panic!("missing user by email"),
        Err(error) => panic!("failed to find user by email: {error:?}"),
    };
    let loaded_by_stripe = match users
        .in_transaction(&mut tx)
        .find_by_stripe_customer_id(&StripeCustomerId::from("cus_postgres_main"))
        .await
    {
        Ok(Some(loaded)) => loaded,
        Ok(None) => panic!("missing user by stripe customer id"),
        Err(error) => panic!("failed to find user by stripe customer id: {error:?}"),
    };

    assert_eq!(user.id(), loaded_by_id.value.id());
    assert_eq!(user.id(), loaded_by_email.value.id());
    assert_eq!(user.id(), loaded_by_stripe.value.id());
    assert!(!loaded_by_id.value.has_marketing_email_consent());
    let account_email = user.email().clone();
    user.grant_marketing_email_consent(&account_email)
        .expect("matching user email should be accepted");
    {
        let intents = SqlxMarketingConsentIntentRepository::new();
        let mut intents = intents.in_transaction(&mut tx);
        let consent_user = intents.find_user_by_id(user.id()).await.unwrap().unwrap();
        assert_eq!(loaded_by_id.version, consent_user.version);
        intents
            .record_user_transition(
                &consent_user,
                true,
                ConsentIntentSource::AuraDoubleOptIn,
                "user-repository-test-consent",
                None,
                time::OffsetDateTime::now_utc(),
            )
            .await
            .expect("consent transition should persist");
    }
    let consented = users
        .in_transaction(&mut tx)
        .find_by_id(user.id())
        .await
        .expect("consented user should load")
        .expect("consented user should exist");
    assert!(consented.value.has_marketing_email_consent());

    user.change_role(UserRole::User);
    user.change_tier(UserTier::Ultimate);
    user.change_stripe_customer_id(None);
    match users
        .in_transaction(&mut tx)
        .update(&user, consented.version)
        .await
    {
        Ok(_) => {}
        Err(error) => panic!("failed to update user: {error:?}"),
    }
    commit(tx).await;

    let mut tx = begin(&unit_of_work).await;
    let updated = match users.in_transaction(&mut tx).find_by_id(user.id()).await {
        Ok(Some(loaded)) => loaded,
        Ok(None) => panic!("missing updated user"),
        Err(error) => panic!("failed to find updated user: {error:?}"),
    };
    commit(tx).await;

    assert_eq!(user.email(), updated.value.email());
    assert_eq!(UserRole::User, updated.value.account().role);
    assert_eq!(UserTier::Ultimate, updated.value.account().tier);
    assert_eq!(None, updated.value.account().stripe_customer_id);
    assert!(updated.value.has_marketing_email_consent());
    assert!(updated.version.into_inner() > consented.version.into_inner());
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_durably_update_and_rehydrate_user_suspension() {
    let pool = get_postgres_client().await;
    let unit_of_work = SqlxUnitOfWork::new(pool);
    let users = SqlxUserRepositoryFactory::new();
    let user = sample_user("postgres-suspension", UserRole::User, None);

    let mut tx = begin(&unit_of_work).await;
    let inserted = match users.in_transaction(&mut tx).insert(&user).await {
        Ok(user) => user,
        Err(error) => panic!("failed to insert user: {error:?}"),
    };
    assert!(!inserted.value.is_suspended());

    let suspended_user = match User::rehydrate(RehydratedUserState {
        id: user.id(),
        email: user.email().clone(),
        marketing_email_consent: user.has_marketing_email_consent(),
        profile: user.profile().clone(),
        preferences: user.preferences().clone(),
        account: user.account().clone(),
        suspended: true,
    }) {
        Ok(user) => user,
        Err(error) => panic!("failed to rehydrate suspended user: {error}"),
    };
    let updated = match users
        .in_transaction(&mut tx)
        .update(&suspended_user, inserted.version)
        .await
    {
        Ok(user) => user,
        Err(error) => panic!("failed to update suspended user: {error:?}"),
    };
    commit(tx).await;

    let mut tx = begin(&unit_of_work).await;
    let rehydrated = match users.in_transaction(&mut tx).find_by_id(user.id()).await {
        Ok(Some(user)) => user,
        Ok(None) => panic!("missing suspended user"),
        Err(error) => panic!("failed to rehydrate suspended user: {error:?}"),
    };
    commit(tx).await;

    assert!(updated.value.is_suspended());
    assert!(rehydrated.value.is_suspended());
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_default_newly_migrated_user_rows_to_no_consent() {
    let pool = get_postgres_client().await;
    let unit_of_work = SqlxUnitOfWork::new(pool.clone());
    let users = SqlxUserRepositoryFactory::new();
    let user_id = UserId::new();
    let email = email("postgres-migrated-consent@example.com");

    if let Err(error) = sqlx::query(
        "INSERT INTO users (user_id, email, tier, role) VALUES ($1, $2, 'FREE', 'USER')",
    )
    .bind(user_id.into_uuid())
    .bind::<&str>(email.as_ref())
    .execute(&pool)
    .await
    {
        panic!("failed to insert row using migration defaults: {error}");
    }

    let mut tx = begin(&unit_of_work).await;
    let migrated = users
        .in_transaction(&mut tx)
        .find_by_id(user_id)
        .await
        .unwrap_or_else(|error| panic!("failed to read migrated row: {error:?}"))
        .unwrap_or_else(|| panic!("migrated row was not found"));
    commit(tx).await;

    assert!(!migrated.value.has_marketing_email_consent());
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn ordinary_user_updates_preserve_consent() {
    let pool = get_postgres_client().await;
    let unit_of_work = SqlxUnitOfWork::new(pool);
    let users = SqlxUserRepositoryFactory::new();
    let mut user = sample_user("postgres-preserve-consent", UserRole::User, None);

    let mut tx = begin(&unit_of_work).await;
    let inserted = users
        .in_transaction(&mut tx)
        .insert(&user)
        .await
        .unwrap_or_else(|error| panic!("failed to insert user: {error:?}"));
    let account_email = user.email().clone();
    user.grant_marketing_email_consent(&account_email)
        .expect("matching user email should be accepted");
    assert!(matches!(
        users
            .in_transaction(&mut tx)
            .update(&user, inserted.version)
            .await,
        Err(UserRepositoryError::ConcurrencyConflict)
    ));
    {
        let intents = SqlxMarketingConsentIntentRepository::new();
        let mut intents = intents.in_transaction(&mut tx);
        let consent_user = intents.find_user_by_id(user.id()).await.unwrap().unwrap();
        assert_eq!(inserted.version, consent_user.version);
        intents
            .record_user_transition(
                &consent_user,
                true,
                ConsentIntentSource::AuraDoubleOptIn,
                "preserve-consent",
                None,
                time::OffsetDateTime::now_utc(),
            )
            .await
            .expect("consent transition should persist");
    }
    let consented = users
        .in_transaction(&mut tx)
        .find_by_id(user.id())
        .await
        .expect("consented user should load")
        .expect("consented user should exist");
    commit(tx).await;

    user.replace_profile(UserProfile {
        first_name: Some(FirstName::from("Grace")),
        last_name: Some(LastName::from("Hopper")),
    })
    .unwrap_or_else(|error| panic!("failed to replace profile: {error}"));
    user.replace_preferences(UserPreferences {
        language: Some(Language::De),
        currency: Some(Currency::Eur),
        measurement_unit: Some(MeasurementUnit::Metric),
        show_unassessed_or_sensitive_content: false,
    });
    user.change_tier(UserTier::Ultimate);
    user.change_stripe_customer_id(Some(StripeCustomerId::from("cus_preserve_consent")));

    let mut tx = begin(&unit_of_work).await;
    let updated = users
        .in_transaction(&mut tx)
        .update(&user, consented.version)
        .await
        .unwrap_or_else(|error| panic!("failed to update profile and account: {error:?}"));
    commit(tx).await;

    assert!(updated.value.has_marketing_email_consent());
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn stale_ordinary_update_cannot_revert_a_concurrent_consent_change() {
    let pool = get_postgres_client().await;
    let unit_of_work = SqlxUnitOfWork::new(pool);
    let users = SqlxUserRepositoryFactory::new();
    let user = sample_user("postgres-consent-race", UserRole::User, None);
    let mut setup_tx = begin(&unit_of_work).await;
    users
        .in_transaction(&mut setup_tx)
        .insert(&user)
        .await
        .unwrap_or_else(|error| panic!("failed to insert user: {error:?}"));
    commit(setup_tx).await;

    let mut stale_tx = begin(&unit_of_work).await;
    let stale = users
        .in_transaction(&mut stale_tx)
        .find_by_id(user.id())
        .await
        .unwrap_or_else(|error| panic!("failed to load ordinary update state: {error:?}"))
        .unwrap_or_else(|| panic!("ordinary update user was not found"));
    let mut stale_profile = stale.value.clone();
    stale_profile
        .replace_profile(UserProfile {
            first_name: Some(FirstName::from("Stale")),
            last_name: None,
        })
        .unwrap_or_else(|error| panic!("failed to create stale profile update: {error}"));

    let mut consent_tx = begin(&unit_of_work).await;
    let current = users
        .in_transaction(&mut consent_tx)
        .find_by_id(user.id())
        .await
        .unwrap_or_else(|error| panic!("failed to load consent state: {error:?}"))
        .unwrap_or_else(|| panic!("consent update user was not found"));
    let mut consented = current.value;
    let consented_email = consented.email().clone();
    consented
        .grant_marketing_email_consent(&consented_email)
        .expect("matching account email should be accepted");
    {
        let intents = SqlxMarketingConsentIntentRepository::new();
        let mut intents = intents.in_transaction(&mut consent_tx);
        let consent_user = intents
            .find_user_by_id(consented.id())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(current.version, consent_user.version);
        intents
            .record_user_transition(
                &consent_user,
                true,
                ConsentIntentSource::AuraDoubleOptIn,
                "consent-race",
                None,
                time::OffsetDateTime::now_utc(),
            )
            .await
            .expect("consent transition should persist");
    }
    commit(consent_tx).await;

    let stale_result = users
        .in_transaction(&mut stale_tx)
        .update(&stale_profile, stale.version)
        .await;
    assert!(matches!(
        stale_result,
        Err(UserRepositoryError::ConcurrencyConflict)
    ));
    commit(stale_tx).await;

    let mut verify_tx = begin(&unit_of_work).await;
    let persisted = users
        .in_transaction(&mut verify_tx)
        .find_by_id(user.id())
        .await
        .unwrap_or_else(|error| panic!("failed to verify consent state: {error:?}"))
        .unwrap_or_else(|| panic!("consent update user disappeared"));
    commit(verify_tx).await;
    assert!(persisted.value.has_marketing_email_consent());
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_report_user_repository_conflicts_and_missing_rows() {
    let pool = get_postgres_client().await;
    let unit_of_work = SqlxUnitOfWork::new(pool);
    let users = SqlxUserRepositoryFactory::new();
    let user = sample_user(
        "postgres-conflict",
        UserRole::User,
        Some("cus_postgres_conflict"),
    );
    let duplicate_email = sample_user("postgres-conflict", UserRole::User, Some("cus_other"));
    let duplicate_stripe = sample_user(
        "postgres-conflict-other",
        UserRole::User,
        Some("cus_postgres_conflict"),
    );

    let mut tx = begin(&unit_of_work).await;
    match users.in_transaction(&mut tx).insert(&user).await {
        Ok(_) => {}
        Err(error) => panic!("failed to insert user: {error:?}"),
    }
    let missing = match users
        .in_transaction(&mut tx)
        .find_by_id(UserId::new())
        .await
    {
        Ok(value) => value,
        Err(error) => panic!("failed missing lookup: {error:?}"),
    };
    assert!(missing.is_none());
    commit(tx).await;

    let mut tx = begin(&unit_of_work).await;
    let email_conflict = users.in_transaction(&mut tx).insert(&duplicate_email).await;
    assert!(matches!(
        email_conflict,
        Err(UserRepositoryError::EmailConflict { source }) if !source.to_string().is_empty()
    ));

    let mut tx = begin(&unit_of_work).await;
    let stripe_conflict = users
        .in_transaction(&mut tx)
        .insert(&duplicate_stripe)
        .await;
    assert!(matches!(
        stripe_conflict,
        Err(UserRepositoryError::StripeCustomerConflict { source }) if !source.to_string().is_empty()
    ));
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_return_existing_user_when_insert_if_absent_replays_user_id() {
    let pool = get_postgres_client().await;
    let unit_of_work = SqlxUnitOfWork::new(pool);
    let users = SqlxUserRepositoryFactory::new();
    let user = sample_user("postgres-idempotent", UserRole::User, None);
    let mut tx = begin(&unit_of_work).await;
    match users.in_transaction(&mut tx).insert_if_absent(&user).await {
        Ok(UserInsertOutcome::Created(_)) => {}
        Ok(UserInsertOutcome::Existing(_)) => panic!("first insert unexpectedly found a user"),
        Err(error) => panic!("failed to create user: {error:?}"),
    }
    commit(tx).await;

    let mut tx = begin(&unit_of_work).await;
    let same_user = users.in_transaction(&mut tx).insert_if_absent(&user).await;
    commit(tx).await;

    assert!(matches!(
        same_user,
        Ok(UserInsertOutcome::Existing(existing)) if existing.value.email() == user.email()
    ));
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_report_user_update_concurrency_conflict() {
    let pool = get_postgres_client().await;
    let unit_of_work = SqlxUnitOfWork::new(pool);
    let users = SqlxUserRepositoryFactory::new();
    let mut user = sample_user("postgres-stale", UserRole::User, None);

    let mut tx = begin(&unit_of_work).await;
    match users.in_transaction(&mut tx).insert(&user).await {
        Ok(_) => {}
        Err(error) => panic!("failed to insert user: {error:?}"),
    }
    let loaded = match users.in_transaction(&mut tx).find_by_id(user.id()).await {
        Ok(Some(loaded)) => loaded,
        Ok(None) => panic!("missing user"),
        Err(error) => panic!("failed to load user: {error:?}"),
    };
    user.change_tier(UserTier::Pro);
    match users
        .in_transaction(&mut tx)
        .update(&user, loaded.version)
        .await
    {
        Ok(_) => {}
        Err(error) => panic!("failed first update: {error:?}"),
    }
    let stale = users
        .in_transaction(&mut tx)
        .update(&user, loaded.version)
        .await;

    assert!(matches!(
        stale,
        Err(UserRepositoryError::ConcurrencyConflict)
    ));
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_reject_unrepresentable_user_version_without_poisoning_transaction() {
    let unit_of_work = SqlxUnitOfWork::new(get_postgres_client().await);
    let users = SqlxUserRepositoryFactory::new();
    let mut user = sample_user("postgres-version-overflow", UserRole::User, None);
    let mut tx = begin(&unit_of_work).await;
    let inserted = users.in_transaction(&mut tx).insert(&user).await.unwrap();
    user.change_tier(UserTier::Pro);

    let invalid_version = UserStorageVersion::try_from(i64::MAX as u64 + 1).unwrap();
    assert!(matches!(
        users
            .in_transaction(&mut tx)
            .update(&user, invalid_version)
            .await,
        Err(UserRepositoryError::InvalidPersistedState { .. })
    ));

    let unchanged = users
        .in_transaction(&mut tx)
        .find_by_id(user.id())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(inserted, unchanged);
    let updated = users
        .in_transaction(&mut tx)
        .update(&user, inserted.version)
        .await
        .unwrap();
    assert_eq!(user, updated.value);
    assert_eq!(inserted.version.next(), updated.version);
    commit(tx).await;
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_rollback_insert_when_transaction_drops() {
    let pool = get_postgres_client().await;
    let unit_of_work = SqlxUnitOfWork::new(pool);
    let users = SqlxUserRepositoryFactory::new();
    let user = sample_user("postgres-rollback", UserRole::User, None);

    let mut tx = begin(&unit_of_work).await;
    match users.in_transaction(&mut tx).insert(&user).await {
        Ok(_) => {}
        Err(error) => panic!("failed to insert rollback user: {error:?}"),
    }
    drop(tx);

    let mut tx = begin(&unit_of_work).await;
    let missing = match users.in_transaction(&mut tx).find_by_id(user.id()).await {
        Ok(value) => value,
        Err(error) => panic!("failed rollback lookup: {error:?}"),
    };
    commit(tx).await;

    assert!(missing.is_none());
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_report_concurrency_conflict_when_updating_missing_user() {
    let pool = get_postgres_client().await;
    let unit_of_work = SqlxUnitOfWork::new(pool);
    let users = SqlxUserRepositoryFactory::new();
    let user = sample_user("postgres-missing-update", UserRole::User, None);

    let mut tx = begin(&unit_of_work).await;
    let result = users
        .in_transaction(&mut tx)
        .update(&user, user_service::ports::UserStorageVersion::INITIAL)
        .await;

    assert!(matches!(
        result,
        Err(UserRepositoryError::ConcurrencyConflict)
    ));
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_delete_user_in_postgres() {
    let pool = get_postgres_client().await;
    let unit_of_work = SqlxUnitOfWork::new(pool);
    let users = SqlxUserRepositoryFactory::new();
    let user = sample_user("postgres-delete", UserRole::User, None);

    let mut tx = begin(&unit_of_work).await;
    match users.in_transaction(&mut tx).insert(&user).await {
        Ok(_) => {}
        Err(error) => panic!("failed to insert user: {error:?}"),
    }
    match users.in_transaction(&mut tx).delete_by_id(user.id()).await {
        Ok(deleted) => assert!(deleted),
        Err(error) => panic!("failed to delete user: {error:?}"),
    }
    let missing = match users.in_transaction(&mut tx).find_by_id(user.id()).await {
        Ok(value) => value,
        Err(error) => panic!("failed deleted lookup: {error:?}"),
    };
    let missing_delete = match users
        .in_transaction(&mut tx)
        .delete_by_id(UserId::new())
        .await
    {
        Ok(value) => value,
        Err(error) => panic!("failed missing delete: {error:?}"),
    };

    assert!(missing.is_none());
    assert!(!missing_delete);
}

fn sample_user(slug: &str, role: UserRole, stripe_customer_id: Option<&str>) -> User {
    match User::create(NewUser {
        id: UserId::new(),
        email: email(&format!("{slug}@example.com")),
        profile: UserProfile {
            first_name: Some(FirstName::from("Ada")),
            last_name: Some(LastName::from("Lovelace")),
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

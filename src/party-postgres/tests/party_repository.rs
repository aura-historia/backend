use application::transaction::{Transaction, UnitOfWork};
use party_core::{
    party::{NewParty, Party, PartyContact},
    party_id::PartyId,
    party_name::PartyName,
};
use party_postgres::SqlxPartyRepositoryFactory;
use party_service::ports::{PartyRepository, PartyRepositoryError, PartyRepositoryFactory};
use platform_postgres::SqlxUnitOfWork;
use serde_email::Email;
use test_api::{IntegrationTestService, aura_integration_test, get_postgres_client};

const BUSINESS_SCHEMA: test_api::Postgres = test_api::Postgres::new("migrations");

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_insert_and_find_party_by_id_and_slug() {
    let pool = get_postgres_client().await;
    let unit_of_work = SqlxUnitOfWork::new(pool);
    let parties = SqlxPartyRepositoryFactory::new();
    let party = sample_party("insert-find");

    let mut tx = begin(&unit_of_work).await;
    let inserted = match parties.in_transaction(&mut tx).insert(&party).await {
        Ok(value) => value,
        Err(error) => panic!("failed to insert party: {error:?}"),
    };
    let by_id = match parties.in_transaction(&mut tx).find_by_id(party.id()).await {
        Ok(Some(value)) => value,
        Ok(None) => panic!("missing party by id"),
        Err(error) => panic!("failed to find party by id: {error:?}"),
    };
    let by_slug = match parties
        .in_transaction(&mut tx)
        .find_by_slug(party.slug_id())
        .await
    {
        Ok(Some(value)) => value,
        Ok(None) => panic!("missing party by slug"),
        Err(error) => panic!("failed to find party by slug: {error:?}"),
    };
    commit(tx).await;

    assert_eq!(party, inserted.party);
    assert_eq!(party.id(), by_id.party.id());
    assert_eq!(party.id(), by_slug.party.id());
    assert_eq!(party.contact().email, by_id.party.contact().email);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_retain_slug_when_renaming_party_and_replace_contact() {
    let pool = get_postgres_client().await;
    let unit_of_work = SqlxUnitOfWork::new(pool);
    let parties = SqlxPartyRepositoryFactory::new();
    let party = sample_party("stable-slug");

    let mut tx = begin(&unit_of_work).await;
    let stored = match parties.in_transaction(&mut tx).insert(&party).await {
        Ok(value) => value,
        Err(error) => panic!("failed to insert party: {error:?}"),
    };
    commit(tx).await;

    let mut renamed = stored.party;
    let original_slug = renamed.slug_id().clone();
    let _ = renamed.rename(PartyName::from("Renamed party"));
    let email = match Email::try_from("updated@example.com") {
        Ok(value) => value,
        Err(error) => panic!("invalid test email: {error}"),
    };
    let _ = renamed.replace_contact(PartyContact {
        phone: Some("+49 30 123456".to_owned()),
        email: Some(email),
    });

    let mut tx = begin(&unit_of_work).await;
    let updated = match parties
        .in_transaction(&mut tx)
        .update(&renamed, stored.version)
        .await
    {
        Ok(value) => value,
        Err(error) => panic!("failed to update party: {error:?}"),
    };
    commit(tx).await;

    assert_eq!(original_slug, *updated.party.slug_id());
    assert_eq!("Renamed party", updated.party.name().as_ref());
    assert_eq!(
        Some("+49 30 123456"),
        updated.party.contact().phone.as_deref()
    );
    assert_eq!(
        Some("updated@example.com"),
        updated
            .party
            .contact()
            .email
            .as_ref()
            .map(ToString::to_string)
            .as_deref()
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_report_slug_conflict_when_inserting_duplicate_party_slug() {
    let pool = get_postgres_client().await;
    let unit_of_work = SqlxUnitOfWork::new(pool);
    let parties = SqlxPartyRepositoryFactory::new();
    let first = sample_party("duplicate-slug");
    let second = sample_party("duplicate-slug");

    let mut tx = begin(&unit_of_work).await;
    match parties.in_transaction(&mut tx).insert(&first).await {
        Ok(_) => {}
        Err(error) => panic!("failed to insert first party: {error:?}"),
    }
    let result = parties.in_transaction(&mut tx).insert(&second).await;

    assert!(matches!(
        result,
        Err(PartyRepositoryError::SlugConflict { .. })
    ));
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_report_concurrency_conflict_for_stale_party_update() {
    let pool = get_postgres_client().await;
    let unit_of_work = SqlxUnitOfWork::new(pool);
    let parties = SqlxPartyRepositoryFactory::new();
    let party = sample_party("concurrency");

    let mut tx = begin(&unit_of_work).await;
    let stored = match parties.in_transaction(&mut tx).insert(&party).await {
        Ok(value) => value,
        Err(error) => panic!("failed to insert party: {error:?}"),
    };
    commit(tx).await;

    let mut updated_party = stored.party.clone();
    let _ = updated_party.rename(PartyName::from("Changed party"));
    let mut tx = begin(&unit_of_work).await;
    match parties
        .in_transaction(&mut tx)
        .update(&updated_party, stored.version)
        .await
    {
        Ok(_) => {}
        Err(error) => panic!("failed to update party: {error:?}"),
    }
    commit(tx).await;

    let mut tx = begin(&unit_of_work).await;
    let stale_result = parties
        .in_transaction(&mut tx)
        .update(&updated_party, stored.version)
        .await;

    assert!(matches!(
        stale_result,
        Err(PartyRepositoryError::ConcurrencyConflict)
    ));
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_reject_invalid_persisted_party_state() {
    let pool = get_postgres_client().await;
    let unit_of_work = SqlxUnitOfWork::new(pool.clone());
    let parties = SqlxPartyRepositoryFactory::new();
    let party = sample_party("invalid-state");

    let mut tx = begin(&unit_of_work).await;
    match parties.in_transaction(&mut tx).insert(&party).await {
        Ok(_) => {}
        Err(error) => panic!("failed to insert party: {error:?}"),
    }
    commit(tx).await;

    match sqlx::query("UPDATE parties SET email = $1 WHERE party_id = $2")
        .bind("invalid-email")
        .bind(uuid::Uuid::from(party.id()))
        .execute(&pool)
        .await
    {
        Ok(_) => {}
        Err(error) => panic!("failed to corrupt party row: {error}"),
    }

    let mut tx = begin(&unit_of_work).await;
    let result = parties.in_transaction(&mut tx).find_by_id(party.id()).await;

    assert!(matches!(
        result,
        Err(PartyRepositoryError::InvalidPersistedState { .. })
    ));
}

fn sample_party(name: &str) -> Party {
    let email = match Email::try_from(format!("{name}@example.com")) {
        Ok(value) => value,
        Err(error) => panic!("invalid test email: {error}"),
    };
    Party::create(NewParty {
        id: PartyId::new(),
        name: PartyName::from(name),
        contact: PartyContact {
            phone: Some("+49 30 123456".to_owned()),
            email: Some(email),
        },
    })
}

async fn begin(unit_of_work: &SqlxUnitOfWork) -> platform_postgres::SqlxTransaction {
    match unit_of_work.begin().await {
        Ok(tx) => tx,
        Err(error) => panic!("failed to begin transaction: {error}"),
    }
}

async fn commit(tx: platform_postgres::SqlxTransaction) {
    if let Err(error) = tx.commit().await {
        panic!("failed to commit transaction: {error}");
    }
}

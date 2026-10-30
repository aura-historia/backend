use application::transaction::{Transaction, UnitOfWork};
use party_core::{
    party::{NewParty, Party, PartyContact},
    party_id::PartyId,
    party_name::PartyName,
    party_slug_id::PartySlugId,
};
use party_postgres::SqlxPartyRepositoryFactory;
use party_service::{
    party_creation::insert_new_party,
    ports::{PartyRepository, PartyRepositoryError, PartyRepositoryFactory},
};
use platform_postgres::SqlxUnitOfWork;
use serde_email::Email;
use test_api::{IntegrationTestService, aura_integration_test, get_postgres_client};

const BUSINESS_SCHEMA: test_api::Postgres = test_api::Postgres::new("migrations");

#[aura_integration_test(services = [BUSINESS_SCHEMA], isolated = true)]
async fn should_insert_and_find_party_by_id_and_slug() {
    let pool = get_postgres_client().await;
    let unit_of_work = SqlxUnitOfWork::new(pool.clone());
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
    let name_search =
        sqlx::query_scalar::<_, String>("SELECT name_search FROM parties WHERE party_id = $1")
            .bind(party.id().into_uuid())
            .fetch_one(&pool)
            .await
            .unwrap_or_else(|error| panic!("read repository-created party name search: {error}"));
    assert_eq!("insert-find", name_search);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA], isolated = true)]
async fn should_insert_same_named_party_under_next_slug_after_conflict_in_same_transaction() {
    let pool = get_postgres_client().await;
    let unit_of_work = SqlxUnitOfWork::new(pool.clone());
    let parties = SqlxPartyRepositoryFactory::new();
    let existing = sample_party("shared-name");
    let same_slug = Party::create(
        NewParty {
            id: PartyId::new(),
            name: party_name("Shared Name"),
            contact: PartyContact::default(),
        },
        existing.slug_id().clone(),
    );

    let mut tx = begin(&unit_of_work).await;
    let mut repository = parties.in_transaction(&mut tx);
    let first = repository.insert(&existing).await;
    let conflict = repository.insert(&same_slug).await;
    let disambiguated = insert_new_party(
        &mut repository,
        NewParty {
            id: PartyId::new(),
            name: party_name("Shared Name"),
            contact: PartyContact::default(),
        },
    )
    .await;
    drop(repository);
    commit(tx).await;

    assert!(first.is_ok());
    assert!(matches!(
        conflict,
        Err(PartyRepositoryError::SlugConflict { .. })
    ));
    assert!(matches!(
        disambiguated,
        Ok(ref stored) if stored.party.slug_id().as_ref() == "shared-name-2"
    ));
    let slugs =
        sqlx::query_scalar::<_, String>("SELECT party_slug_id FROM parties ORDER BY party_slug_id")
            .fetch_all(&pool)
            .await
            .unwrap_or_else(|error| panic!("read committed party slugs: {error}"));
    assert_eq!(vec!["shared-name", "shared-name-2"], slugs);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA], isolated = true)]
async fn should_wait_for_uncommitted_slug_holder_and_take_next_slug_after_it_commits() {
    let pool = get_postgres_client().await;
    let unit_of_work = SqlxUnitOfWork::new(pool.clone());
    let parties = SqlxPartyRepositoryFactory::new();

    let mut holder = begin(&unit_of_work).await;
    let held = parties
        .in_transaction(&mut holder)
        .insert(&sample_party("concurrent-name"))
        .await;
    assert!(held.is_ok());

    let mut waiter = begin(&unit_of_work).await;
    let waiter_pid = sqlx::query_scalar::<_, i32>("SELECT pg_backend_pid()")
        .fetch_one(waiter.connection())
        .await
        .unwrap_or_else(|error| panic!("read waiter backend: {error}"));
    let waiting = test_api::spawn_owned(async move {
        let inserted = insert_new_party(
            &mut SqlxPartyRepositoryFactory::new().in_transaction(&mut waiter),
            NewParty {
                id: PartyId::new(),
                name: party_name("Concurrent Name"),
                contact: PartyContact::default(),
            },
        )
        .await;
        commit(waiter).await;
        inserted
    });
    wait_until_backend_waits_for_lock(&pool, waiter_pid).await;
    commit(holder).await;

    let inserted = waiting
        .await
        .unwrap_or_else(|error| panic!("join waiting party insert: {error}"));
    assert!(matches!(
        inserted,
        Ok(ref stored) if stored.party.slug_id().as_ref() == "concurrent-name-2"
    ));
}

#[aura_integration_test(services = [BUSINESS_SCHEMA], isolated = true)]
async fn should_retain_slug_when_renaming_party_and_replace_contact() {
    let pool = get_postgres_client().await;
    let unit_of_work = SqlxUnitOfWork::new(pool.clone());
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
    let _ = renamed.rename(party_name("Renamed party"));
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
    let name_search =
        sqlx::query_scalar::<_, String>("SELECT name_search FROM parties WHERE party_id = $1")
            .bind(updated.party.id().into_uuid())
            .fetch_one(&pool)
            .await
            .unwrap_or_else(|error| panic!("read repository-renamed party name search: {error}"));
    assert_eq!("renamed party", name_search);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA], isolated = true)]
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
    let _ = updated_party.rename(party_name("Changed party"));
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

#[aura_integration_test(services = [BUSINESS_SCHEMA], isolated = true)]
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
        .bind(party.id().into_uuid())
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

#[aura_integration_test(services = [BUSINESS_SCHEMA], isolated = true)]
async fn should_reject_wrong_version_persisted_party_id() {
    let pool = get_postgres_client().await;
    let party_id = uuid::Uuid::new_v4();
    let slug_id = PartySlugId::raw("wrong-version-party")
        .unwrap_or_else(|error| panic!("valid Party slug fixture: {error}"));
    sqlx::query("INSERT INTO parties (party_id, party_slug_id, name) VALUES ($1, $2, $3)")
        .bind(party_id)
        .bind(slug_id.as_ref())
        .bind("Wrong version Party")
        .execute(&pool)
        .await
        .unwrap_or_else(|error| panic!("insert wrong-version Party row: {error}"));

    let unit_of_work = SqlxUnitOfWork::new(pool);
    let mut tx = begin(&unit_of_work).await;
    let result = SqlxPartyRepositoryFactory::new()
        .in_transaction(&mut tx)
        .find_by_slug(&slug_id)
        .await;

    assert!(matches!(
        result,
        Err(PartyRepositoryError::InvalidPersistedState { .. })
    ));
}

#[aura_integration_test(services = [BUSINESS_SCHEMA], isolated = true)]
async fn should_reject_invalid_persisted_party_name() {
    let pool = get_postgres_client().await;
    let party_id = PartyId::new();

    let inserted =
        sqlx::query("INSERT INTO parties (party_id, party_slug_id, name) VALUES ($1, $2, $3)")
            .bind(party_id.into_uuid())
            .bind("invalid-name")
            .bind("Valid name")
            .execute(&pool)
            .await;
    assert!(inserted.is_ok());

    let corrupted = sqlx::query("UPDATE parties SET name = $1 WHERE party_id = $2")
        .bind("\u{2003}\u{00a0}")
        .bind(party_id.into_uuid())
        .execute(&pool)
        .await;
    assert!(corrupted.is_ok());

    let unit_of_work = SqlxUnitOfWork::new(pool);
    let mut tx = begin(&unit_of_work).await;
    let result = SqlxPartyRepositoryFactory::new()
        .in_transaction(&mut tx)
        .find_by_id(party_id)
        .await;

    assert!(matches!(
        result,
        Err(PartyRepositoryError::InvalidPersistedState { .. })
    ));
}

#[aura_integration_test(services = [BUSINESS_SCHEMA], isolated = true)]
async fn should_enforce_party_name_and_slug_schema_constraints() {
    let pool = get_postgres_client().await;

    let blank_name =
        sqlx::query("INSERT INTO parties (party_id, party_slug_id, name) VALUES ($1, $2, $3)")
            .bind(uuid::Uuid::now_v7())
            .bind("valid-slug")
            .bind("   ")
            .execute(&pool)
            .await;
    assert!(blank_name.is_err());

    let oversized_name =
        sqlx::query("INSERT INTO parties (party_id, party_slug_id, name) VALUES ($1, $2, $3)")
            .bind(uuid::Uuid::now_v7())
            .bind("another-valid-slug")
            .bind("é".repeat(128))
            .execute(&pool)
            .await;
    assert!(oversized_name.is_err());

    let blank_slug =
        sqlx::query("INSERT INTO parties (party_id, party_slug_id, name) VALUES ($1, $2, $3)")
            .bind(uuid::Uuid::now_v7())
            .bind("")
            .bind("Valid name")
            .execute(&pool)
            .await;
    assert!(blank_slug.is_err());
}

#[aura_integration_test(services = [BUSINESS_SCHEMA], isolated = true)]
async fn should_install_and_maintain_normalized_party_and_listing_source_names() {
    let pool = get_postgres_client().await;
    let raw_party_name = "\u{2003}Mu\u{308}ller\tKunst\u{00a0}Handel\u{202f}\u{3000}";
    let expected_party_name_search = "muller kunst handel";
    let party_id = uuid::Uuid::now_v7();

    let extensions = sqlx::query_scalar::<_, String>(
        "SELECT n.nspname || '.' || e.extname FROM pg_extension e JOIN pg_namespace n ON n.oid = e.extnamespace WHERE e.extname IN ('pg_trgm', 'unaccent') ORDER BY e.extname",
    )
    .fetch_all(&pool)
    .await
    .unwrap_or_else(|error| panic!("read public search extensions: {error}"));
    assert_eq!(vec!["public.pg_trgm", "public.unaccent"], extensions);

    let normalized = sqlx::query_scalar::<_, String>("SELECT public.aura_search_name($1)")
        .bind(raw_party_name)
        .fetch_one(&pool)
        .await
        .unwrap_or_else(|error| panic!("normalize party name: {error}"));
    assert_eq!(expected_party_name_search, normalized);
    let cjk_and_cyrillic = sqlx::query_scalar::<_, String>("SELECT public.aura_search_name($1)")
        .bind(" 東京\u{3000}АНТИК ")
        .fetch_one(&pool)
        .await
        .unwrap_or_else(|error| panic!("normalize CJK and Cyrillic name: {error}"));
    assert_eq!("東京 антик", cjk_and_cyrillic);

    sqlx::query("INSERT INTO parties (party_id, party_slug_id, name) VALUES ($1, $2, $3)")
        .bind(party_id)
        .bind("normalized-party")
        .bind(raw_party_name)
        .execute(&pool)
        .await
        .unwrap_or_else(|error| panic!("insert normalized party fixture: {error}"));

    let party_name_search =
        sqlx::query_scalar::<_, String>("SELECT name_search FROM parties WHERE party_id = $1")
            .bind(party_id)
            .fetch_one(&pool)
            .await
            .unwrap_or_else(|error| panic!("read party name search: {error}"));
    assert_eq!(expected_party_name_search, party_name_search);

    sqlx::query("UPDATE parties SET name_search = 'drift' WHERE party_id = $1")
        .bind(party_id)
        .execute(&pool)
        .await
        .unwrap_or_else(|error| panic!("attempt party search drift: {error}"));
    let party_name_search =
        sqlx::query_scalar::<_, String>("SELECT name_search FROM parties WHERE party_id = $1")
            .bind(party_id)
            .fetch_one(&pool)
            .await
            .unwrap_or_else(|error| panic!("read recomputed party name search: {error}"));
    assert_eq!(expected_party_name_search, party_name_search);

    let source_id = uuid::Uuid::now_v7();
    sqlx::query(
        "INSERT INTO listing_sources (listing_source_id, listing_source_slug_id, name, operator_party_id) VALUES ($1, $2, $3, $4)",
    )
    .bind(source_id)
    .bind(format!("normalized-source-{source_id}"))
    .bind(" Café\u{3000}MÜLLER ")
    .bind(party_id)
    .execute(&pool)
    .await
    .unwrap_or_else(|error| panic!("insert normalized listing source fixture: {error}"));

    sqlx::query("UPDATE listing_sources SET name_search = 'drift' WHERE listing_source_id = $1")
        .bind(source_id)
        .execute(&pool)
        .await
        .unwrap_or_else(|error| panic!("attempt listing source search drift: {error}"));
    let source_name_search = sqlx::query_scalar::<_, String>(
        "SELECT name_search FROM listing_sources WHERE listing_source_id = $1",
    )
    .bind(source_id)
    .fetch_one(&pool)
    .await
    .unwrap_or_else(|error| panic!("read recomputed listing source name search: {error}"));
    assert_eq!("cafe muller", source_name_search);

    sqlx::query("UPDATE listing_sources SET name = $1 WHERE listing_source_id = $2")
        .bind("Renamed\u{00a0}Source")
        .bind(source_id)
        .execute(&pool)
        .await
        .unwrap_or_else(|error| panic!("rename listing source fixture: {error}"));
    let renamed_source_name_search = sqlx::query_scalar::<_, String>(
        "SELECT name_search FROM listing_sources WHERE listing_source_id = $1",
    )
    .bind(source_id)
    .fetch_one(&pool)
    .await
    .unwrap_or_else(|error| panic!("read renamed listing source name search: {error}"));
    assert_eq!("renamed source", renamed_source_name_search);

    sqlx::query("UPDATE parties SET name = $1 WHERE party_id = $2")
        .bind("Renamed\u{00a0}Operator")
        .bind(party_id)
        .execute(&pool)
        .await
        .unwrap_or_else(|error| panic!("rename party fixture: {error}"));
    let renamed_party_name_search =
        sqlx::query_scalar::<_, String>("SELECT name_search FROM parties WHERE party_id = $1")
            .bind(party_id)
            .fetch_one(&pool)
            .await
            .unwrap_or_else(|error| panic!("read renamed party name search: {error}"));
    assert_eq!("renamed operator", renamed_party_name_search);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA], isolated = true)]
async fn should_delete_unused_party_after_locking_and_leave_unrelated_party() {
    let pool = get_postgres_client().await;
    let unit_of_work = SqlxUnitOfWork::new(pool);
    let parties = SqlxPartyRepositoryFactory::new();
    let target = sample_party("delete-target");
    let unrelated = sample_party("delete-unrelated");

    let mut tx = begin(&unit_of_work).await;
    let stored = match parties.in_transaction(&mut tx).insert(&target).await {
        Ok(value) => value,
        Err(error) => panic!("failed to insert target party: {error:?}"),
    };
    if let Err(error) = parties.in_transaction(&mut tx).insert(&unrelated).await {
        panic!("failed to insert unrelated party: {error:?}");
    }
    if let Err(error) = parties
        .in_transaction(&mut tx)
        .find_by_id_for_update(target.id())
        .await
    {
        panic!("failed to lock target party: {error:?}");
    }
    match parties
        .in_transaction(&mut tx)
        .find_deletion_blocker(target.id())
        .await
    {
        Ok(None) => {}
        Ok(Some(blocker)) => panic!("unexpected deletion blocker: {blocker:?}"),
        Err(error) => panic!("failed to read deletion blocker: {error:?}"),
    }
    if let Err(error) = parties
        .in_transaction(&mut tx)
        .delete_unused(target.id(), stored.version)
        .await
    {
        panic!("failed to delete unused party: {error:?}");
    }
    commit(tx).await;

    let mut tx = begin(&unit_of_work).await;
    assert!(matches!(
        parties
            .in_transaction(&mut tx)
            .find_by_id(target.id())
            .await,
        Ok(None)
    ));
    assert!(matches!(
        parties
            .in_transaction(&mut tx)
            .find_by_id(unrelated.id())
            .await,
        Ok(Some(_))
    ));
}

#[aura_integration_test(services = [BUSINESS_SCHEMA], isolated = true)]
async fn should_report_listing_source_and_active_or_dissolved_partnership_blockers() {
    let pool = get_postgres_client().await;
    let unit_of_work = SqlxUnitOfWork::new(pool.clone());
    let parties = SqlxPartyRepositoryFactory::new();
    let source_party = sample_party("source-blocker");
    let active_party = sample_party("active-partnership-blocker");
    let dissolved_party = sample_party("dissolved-partnership-blocker");

    let mut tx = begin(&unit_of_work).await;
    for party in [&source_party, &active_party, &dissolved_party] {
        if let Err(error) = parties.in_transaction(&mut tx).insert(party).await {
            panic!("failed to insert party: {error:?}");
        }
    }
    commit(tx).await;

    let source_id = uuid::Uuid::now_v7();
    for (sql, party_id) in [(
        "INSERT INTO listing_sources (listing_source_id, listing_source_slug_id, name, operator_party_id) VALUES ($1, $2, $3, $4)",
        source_party.id(),
    )] {
        if let Err(error) = sqlx::query(sql)
            .bind(source_id)
            .bind(format!("source-{source_id}"))
            .bind("Source blocker")
            .bind(party_id.into_uuid())
            .execute(&pool)
            .await
        {
            panic!("failed to insert listing source blocker: {error}");
        }
    }
    for (party, state) in [(&active_party, "ACTIVE"), (&dissolved_party, "DISSOLVED")] {
        if let Err(error) = sqlx::query(
            "INSERT INTO partnerships (partnership_id, party_id, business_state) VALUES ($1, $2, $3)",
        )
        .bind(uuid::Uuid::now_v7())
        .bind(party.id().into_uuid())
        .bind(state)
        .execute(&pool)
        .await
        {
            panic!("failed to insert partnership blocker: {error}");
        }
    }

    let mut tx = begin(&unit_of_work).await;
    for (party_id, expected) in [
        (
            source_party.id(),
            party_service::ports::PartyDeletionBlocker::ListingSources,
        ),
        (
            active_party.id(),
            party_service::ports::PartyDeletionBlocker::Partnership,
        ),
        (
            dissolved_party.id(),
            party_service::ports::PartyDeletionBlocker::Partnership,
        ),
    ] {
        let locked = parties
            .in_transaction(&mut tx)
            .find_by_id_for_update(party_id)
            .await;
        assert!(matches!(locked, Ok(Some(_))));
        match parties
            .in_transaction(&mut tx)
            .find_deletion_blocker(party_id)
            .await
        {
            Ok(Some(blocker)) => assert_eq!(expected, blocker),
            Ok(None) => panic!("expected party deletion blocker"),
            Err(error) => panic!("failed to read deletion blocker: {error:?}"),
        }
    }
}

#[aura_integration_test(services = [BUSINESS_SCHEMA], isolated = true)]
async fn should_reject_stale_and_direct_delete_of_party_with_restrictive_references() {
    let pool = get_postgres_client().await;
    let unit_of_work = SqlxUnitOfWork::new(pool.clone());
    let parties = SqlxPartyRepositoryFactory::new();
    let stale_party = sample_party("stale-delete");
    let restricted_party = sample_party("restricted-delete");

    let mut tx = begin(&unit_of_work).await;
    let stale = match parties.in_transaction(&mut tx).insert(&stale_party).await {
        Ok(value) => value,
        Err(error) => panic!("failed to insert stale party: {error:?}"),
    };
    if let Err(error) = parties
        .in_transaction(&mut tx)
        .insert(&restricted_party)
        .await
    {
        panic!("failed to insert restrictive party: {error:?}");
    }
    commit(tx).await;

    let mut changed = stale.party.clone();
    let _ = changed.rename(party_name("stale-delete-renamed"));
    let mut tx = begin(&unit_of_work).await;
    if let Err(error) = parties
        .in_transaction(&mut tx)
        .update(&changed, stale.version)
        .await
    {
        panic!("failed to update stale party: {error:?}");
    }
    commit(tx).await;
    let mut tx = begin(&unit_of_work).await;
    assert!(matches!(
        parties
            .in_transaction(&mut tx)
            .delete_unused(stale_party.id(), stale.version)
            .await,
        Err(PartyRepositoryError::ConcurrencyConflict)
    ));

    let partnership_id = uuid::Uuid::now_v7();
    if let Err(error) =
        sqlx::query("INSERT INTO partnerships (partnership_id, party_id) VALUES ($1, $2)")
            .bind(partnership_id)
            .bind(restricted_party.id().into_uuid())
            .execute(&pool)
            .await
    {
        panic!("failed to insert restrictive partnership: {error}");
    }
    let direct_delete = sqlx::query("DELETE FROM parties WHERE party_id = $1")
        .bind(restricted_party.id().into_uuid())
        .execute(&pool)
        .await;
    assert!(direct_delete.is_err());
    let partnership_remains =
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM partnerships WHERE partnership_id = $1")
            .bind(partnership_id)
            .fetch_one(&pool)
            .await;
    match partnership_remains {
        Ok(count) => assert_eq!(1, count),
        Err(error) => panic!("failed to verify retained partnership: {error}"),
    }
}

async fn wait_until_backend_waits_for_lock(pool: &sqlx::PgPool, backend_pid: i32) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        let wait_event_type = sqlx::query_scalar::<_, String>(
            "SELECT COALESCE(wait_event_type, '') FROM pg_stat_activity WHERE pid = $1",
        )
        .bind(backend_pid)
        .fetch_optional(pool)
        .await
        .unwrap_or_else(|error| panic!("inspect waiting backend state: {error}"));
        if wait_event_type.as_deref() == Some("Lock") {
            return;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "concurrent party insert did not wait for the uncommitted slug holder"
        );
        tokio::task::yield_now().await;
    }
}

fn party_name(value: &str) -> PartyName {
    PartyName::try_from(value).unwrap_or_else(|error| panic!("invalid test party name: {error}"))
}

fn sample_party(name: &str) -> Party {
    let email = match Email::try_from(format!("{name}@example.com")) {
        Ok(value) => value,
        Err(error) => panic!("invalid test email: {error}"),
    };
    let slug_id =
        PartySlugId::raw(name).unwrap_or_else(|error| panic!("invalid test party slug: {error}"));
    Party::create(
        NewParty {
            id: PartyId::new(),
            name: party_name(name),
            contact: PartyContact {
                phone: Some("+49 30 123456".to_owned()),
                email: Some(email),
            },
        },
        slug_id,
    )
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

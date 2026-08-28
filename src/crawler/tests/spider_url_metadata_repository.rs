use crawler::spider::classification::url_metadata::{UrlClass, UrlPresence};
use crawler::spider::classification::url_metadata_repository::{
    UrlMetadataRepository, UrlMetadataRepositoryImpl,
};
use listing_source_core::ListingSourceId;
use test_api::*;

const POSTGRES: Postgres = Postgres::new("src/crawler/migrations");
use url::Url;

/// Helper: inserts a shop + domain and returns the generated domain_id.
async fn insert_shop_with_domain(
    pool: &sqlx::PgPool,
    listing_source_id_uuid: uuid::Uuid,
    domain: &str,
) -> uuid::Uuid {
    sqlx::query("INSERT INTO listing_sources (listing_source_id, created, updated) VALUES ($1, NOW(), NOW())")
        .bind(listing_source_id_uuid)
        .execute(pool)
        .await
        .unwrap();

    insert_domain_for_shop(pool, listing_source_id_uuid, domain).await
}

/// Helper: inserts an additional domain row for an already-existing shop.
async fn insert_domain_for_shop(
    pool: &sqlx::PgPool,
    listing_source_id_uuid: uuid::Uuid,
    domain: &str,
) -> uuid::Uuid {
    let row: (uuid::Uuid,) = sqlx::query_as(
        "INSERT INTO listing_source_domains (listing_source_id, listing_source_domain) VALUES ($1, $2) RETURNING domain_id",
    )
    .bind(listing_source_id_uuid)
    .bind(domain)
    .fetch_one(pool)
    .await
    .unwrap();

    row.0
}

// ---------------------------------------------------------------------------
// upsert_link — INSERT path
// ---------------------------------------------------------------------------

#[serial]
#[aura_integration_test(services = [POSTGRES])]
async fn should_insert_new_url_with_present_presence_when_url_does_not_exist() {
    let pool = get_postgres_client().await;
    let repository = UrlMetadataRepositoryImpl::new(pool.clone());

    let listing_source_id: ListingSourceId = uuid::Uuid::new_v4().into();
    let listing_source_id_uuid: uuid::Uuid = listing_source_id.into();
    let domain_id = insert_shop_with_domain(&pool, listing_source_id_uuid, "example.com").await;

    let url = Url::parse("https://example.com/product/123").unwrap();
    let url_class = UrlClass::ProductListing;
    let result = repository
        .upsert_link(&listing_source_id, &domain_id, &url, &url_class)
        .await
        .unwrap();

    assert_eq!(result.url, url);
    assert_eq!(result.url_class, url_class);
    assert_eq!(result.state, UrlPresence::Present);
    assert_eq!(result.domain_id, domain_id);
}

// ---------------------------------------------------------------------------
// upsert_link — UPDATE (conflict) path, same domain
// ---------------------------------------------------------------------------

#[serial]
#[aura_integration_test(services = [POSTGRES])]
async fn should_update_existing_url_when_url_already_exists() {
    let pool = get_postgres_client().await;
    let repository = UrlMetadataRepositoryImpl::new(pool.clone());

    let listing_source_id: ListingSourceId = uuid::Uuid::new_v4().into();
    let listing_source_id_uuid: uuid::Uuid = listing_source_id.into();
    let domain_id = insert_shop_with_domain(&pool, listing_source_id_uuid, "example.com").await;

    let url = Url::parse("https://example.com/product/123").unwrap();
    let old_class = UrlClass::Other;

    repository
        .upsert_link(&listing_source_id, &domain_id, &url, &old_class)
        .await
        .unwrap();

    let new_class = UrlClass::ProductListing;

    let result2 = repository
        .upsert_link(&listing_source_id, &domain_id, &url, &new_class)
        .await
        .unwrap();

    assert_eq!(result2.url, url);
    assert_eq!(result2.url_class, new_class);
    assert_eq!(result2.state, UrlPresence::Present);
    assert_eq!(result2.domain_id, domain_id);
}

// ---------------------------------------------------------------------------
// upsert_link — UPDATE path reassigns domain_id when domain changes
// ---------------------------------------------------------------------------

#[serial]
#[aura_integration_test(services = [POSTGRES])]
async fn should_update_domain_id_when_url_is_upserted_with_different_domain() {
    let pool = get_postgres_client().await;
    let repository = UrlMetadataRepositoryImpl::new(pool.clone());

    // Two listing_sources, each owning one domain — same hostname so we can reuse the URL key.
    let listing_source_id_a: ListingSourceId = uuid::Uuid::new_v4().into();
    let listing_source_id_b: ListingSourceId = uuid::Uuid::new_v4().into();
    let shop_a_uuid: uuid::Uuid = listing_source_id_a.into();
    let shop_b_uuid: uuid::Uuid = listing_source_id_b.into();

    // Both listing_sources get the same hostname so the URL key (TEXT PRIMARY KEY) is the
    // same — this exercises the ON CONFLICT ... DO UPDATE SET domain_id = EXCLUDED.domain_id path.
    let domain_id_a = insert_shop_with_domain(&pool, shop_a_uuid, "shared-host.example.com").await;
    // Give the second shop a *different* domain row that maps to the same
    // host alias.  We need a distinct domain string for the UNIQUE constraint on
    // listing_source_domains.listing_source_domain, so we use a sub-domain.
    let domain_id_b =
        insert_shop_with_domain(&pool, shop_b_uuid, "alias.shared-host.example.com").await;

    let url = Url::parse("https://shared-host.example.com/product/1").unwrap();
    let class = UrlClass::ProductListing;

    // First upsert — domain_id_a
    let r1 = repository
        .upsert_link(&listing_source_id_a, &domain_id_a, &url, &class)
        .await
        .unwrap();
    assert_eq!(r1.domain_id, domain_id_a);

    // Second upsert with the same URL but domain_id_b — ON CONFLICT should update domain_id
    let r2 = repository
        .upsert_link(&listing_source_id_b, &domain_id_b, &url, &class)
        .await
        .unwrap();
    assert_eq!(
        r2.domain_id, domain_id_b,
        "domain_id should be updated on conflict"
    );
}

// ---------------------------------------------------------------------------
// upsert_link — FK violation when domain_id does not exist
// ---------------------------------------------------------------------------

#[serial]
#[aura_integration_test(services = [POSTGRES])]
async fn should_return_error_when_domain_id_does_not_exist_for_upsert_link() {
    let pool = get_postgres_client().await;

    // Insert a shop but do NOT create a listing_source_domains row.
    let listing_source_id: ListingSourceId = uuid::Uuid::new_v4().into();
    let listing_source_id_uuid: uuid::Uuid = listing_source_id.into();
    sqlx::query("INSERT INTO listing_sources (listing_source_id, created, updated) VALUES ($1, NOW(), NOW())")
        .bind(listing_source_id_uuid)
        .execute(&pool)
        .await
        .unwrap();

    let repository = UrlMetadataRepositoryImpl::new(pool.clone());
    let bogus_domain_id = uuid::Uuid::new_v4(); // no matching listing_source_domains row

    let url = Url::parse("https://example.com/product/fk-test").unwrap();
    let result = repository
        .upsert_link(
            &listing_source_id,
            &bogus_domain_id,
            &url,
            &UrlClass::ProductListing,
        )
        .await;

    assert!(result.is_err(), "expected FK violation error but got Ok");
}

// ---------------------------------------------------------------------------
// mark_as_scraped — domain_id preserved in returned record
// ---------------------------------------------------------------------------

#[serial]
#[aura_integration_test(services = [POSTGRES])]
async fn should_update_last_scraped_timestamp_when_marking_as_scraped() {
    let pool = get_postgres_client().await;
    let repository = UrlMetadataRepositoryImpl::new(pool.clone());

    let listing_source_id: ListingSourceId = uuid::Uuid::new_v4().into();
    let listing_source_id_uuid: uuid::Uuid = listing_source_id.into();
    let domain_id = insert_shop_with_domain(&pool, listing_source_id_uuid, "example.com").await;

    let url = Url::parse("https://example.com/product/123").unwrap();
    let url_class = UrlClass::ProductListing;
    repository
        .upsert_link(&listing_source_id, &domain_id, &url, &url_class)
        .await
        .unwrap();

    let marked = repository
        .mark_as_scraped(&listing_source_id, &url, "dummy_hash")
        .await
        .unwrap();

    assert!(marked.last_scraped.is_some());
    assert_eq!(
        marked.domain_id, domain_id,
        "domain_id should be returned by mark_as_scraped"
    );
}

// ---------------------------------------------------------------------------
// set_presence — domain_id preserved in returned record
// ---------------------------------------------------------------------------

#[serial]
#[aura_integration_test(services = [POSTGRES])]
async fn should_update_presence_when_setting_new_presence() {
    let pool = get_postgres_client().await;
    let repository = UrlMetadataRepositoryImpl::new(pool.clone());

    let listing_source_id: ListingSourceId = uuid::Uuid::new_v4().into();
    let listing_source_id_uuid: uuid::Uuid = listing_source_id.into();
    let domain_id = insert_shop_with_domain(&pool, listing_source_id_uuid, "example.com").await;

    let url = Url::parse("https://example.com/product/123").unwrap();
    let url_class = UrlClass::ProductListing;
    let result = repository
        .upsert_link(&listing_source_id, &domain_id, &url, &url_class)
        .await
        .unwrap();

    assert_eq!(result.state, UrlPresence::Present);

    let marked = repository
        .set_presence(&listing_source_id, &url, &UrlPresence::Present)
        .await
        .unwrap();

    assert_eq!(marked.state, UrlPresence::Present);
    assert_eq!(
        marked.domain_id, domain_id,
        "domain_id should be returned by set_presence"
    );
}

// ---------------------------------------------------------------------------
// upsert_links_batch — INSERT path: domain_id asserted on every record
// ---------------------------------------------------------------------------

#[serial]
#[aura_integration_test(services = [POSTGRES])]
async fn should_upsert_multiple_links_when_inserting_batch() {
    let pool = get_postgres_client().await;
    let repository = UrlMetadataRepositoryImpl::new(pool.clone());

    let listing_source_id: ListingSourceId = uuid::Uuid::new_v4().into();
    let listing_source_id_uuid: uuid::Uuid = listing_source_id.into();
    let domain_id = insert_shop_with_domain(&pool, listing_source_id_uuid, "example.com").await;

    let urls = vec![
        Url::parse("https://example.com/product/1").unwrap(),
        Url::parse("https://example.com/product/2").unwrap(),
    ];
    let classes = vec![UrlClass::ProductListing, UrlClass::ProductListing];

    let inserted = repository
        .upsert_links_batch(&listing_source_id, &domain_id, &urls, &classes)
        .await
        .unwrap();

    assert_eq!(inserted.len(), 2);
    assert!(inserted.iter().any(|r| r.url == urls[0]));
    assert!(inserted.iter().any(|r| r.url == urls[1]));
    // Every returned record must carry the correct domain_id
    assert!(
        inserted.iter().all(|r| r.domain_id == domain_id),
        "all batch-inserted records should have domain_id = {domain_id}"
    );

    let updated = repository
        .upsert_links_batch(&listing_source_id, &domain_id, &urls, &classes)
        .await
        .unwrap();

    assert_eq!(updated.len(), 2);
    assert!(updated.iter().any(|r| r.url == urls[0]));
    assert!(updated.iter().any(|r| r.url == urls[1]));
    // domain_id must survive the ON CONFLICT update path too
    assert!(
        updated.iter().all(|r| r.domain_id == domain_id),
        "all batch-updated records should still have domain_id = {domain_id}"
    );
}

// ---------------------------------------------------------------------------
// upsert_links_batch — conflict path reassigns domain_id
// ---------------------------------------------------------------------------

#[serial]
#[aura_integration_test(services = [POSTGRES])]
async fn should_update_domain_id_in_batch_when_url_is_upserted_under_different_domain() {
    let pool = get_postgres_client().await;
    let repository = UrlMetadataRepositoryImpl::new(pool.clone());

    let listing_source_id_a: ListingSourceId = uuid::Uuid::new_v4().into();
    let listing_source_id_b: ListingSourceId = uuid::Uuid::new_v4().into();

    let domain_id_a = insert_shop_with_domain(
        &pool,
        listing_source_id_a.into(),
        "batch-domain-a.example.com",
    )
    .await;
    let domain_id_b = insert_shop_with_domain(
        &pool,
        listing_source_id_b.into(),
        "batch-domain-b.example.com",
    )
    .await;

    let urls = vec![
        Url::parse("https://batch-domain-a.example.com/p/1").unwrap(),
        Url::parse("https://batch-domain-a.example.com/p/2").unwrap(),
    ];
    let classes = vec![UrlClass::ProductListing, UrlClass::ProductListing];
    // First batch — domain_id_a
    let first = repository
        .upsert_links_batch(&listing_source_id_a, &domain_id_a, &urls, &classes)
        .await
        .unwrap();
    assert!(first.iter().all(|r| r.domain_id == domain_id_a));

    // Second batch — same URLs, domain_id_b (ON CONFLICT DO UPDATE SET domain_id = EXCLUDED.domain_id)
    let second = repository
        .upsert_links_batch(&listing_source_id_b, &domain_id_b, &urls, &classes)
        .await
        .unwrap();
    assert!(
        second.iter().all(|r| r.domain_id == domain_id_b),
        "domain_id should be updated to domain_id_b on conflict"
    );
}

// ---------------------------------------------------------------------------
// upsert_links_batch — FK violation when domain_id does not exist
// ---------------------------------------------------------------------------

#[serial]
#[aura_integration_test(services = [POSTGRES])]
async fn should_return_error_when_domain_id_does_not_exist_for_upsert_links_batch() {
    let pool = get_postgres_client().await;

    let listing_source_id: ListingSourceId = uuid::Uuid::new_v4().into();
    let listing_source_id_uuid: uuid::Uuid = listing_source_id.into();
    sqlx::query("INSERT INTO listing_sources (listing_source_id, created, updated) VALUES ($1, NOW(), NOW())")
        .bind(listing_source_id_uuid)
        .execute(&pool)
        .await
        .unwrap();

    let repository = UrlMetadataRepositoryImpl::new(pool.clone());
    let bogus_domain_id = uuid::Uuid::new_v4();

    let urls = vec![Url::parse("https://example.com/product/fk-batch").unwrap()];
    let classes = vec![UrlClass::ProductListing];
    let result = repository
        .upsert_links_batch(&listing_source_id, &bogus_domain_id, &urls, &classes)
        .await;

    assert!(
        result.is_err(),
        "expected FK violation error for batch upsert but got Ok"
    );
}

// ---------------------------------------------------------------------------
// ON DELETE CASCADE — deleting a listing_source_domains row removes child listing_source_urls rows
// ---------------------------------------------------------------------------

#[serial]
#[aura_integration_test(services = [POSTGRES])]
async fn should_delete_urls_when_domain_is_deleted() {
    let pool = get_postgres_client().await;
    let repository = UrlMetadataRepositoryImpl::new(pool.clone());

    let listing_source_id: ListingSourceId = uuid::Uuid::new_v4().into();
    let listing_source_id_uuid: uuid::Uuid = listing_source_id.into();
    let domain_id =
        insert_shop_with_domain(&pool, listing_source_id_uuid, "cascade.example.com").await;

    // Insert two URLs under this domain
    let urls = vec![
        Url::parse("https://cascade.example.com/product/1").unwrap(),
        Url::parse("https://cascade.example.com/product/2").unwrap(),
    ];
    for url in &urls {
        repository
            .upsert_link(
                &listing_source_id,
                &domain_id,
                url,
                &UrlClass::ProductListing,
            )
            .await
            .unwrap();
    }

    // Confirm they exist
    let count_before: (i64,) =
        sqlx::query_as("SELECT COUNT(*) FROM listing_source_urls WHERE domain_id = $1")
            .bind(domain_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        count_before.0, 2,
        "expected 2 listing_source_urls rows before cascade delete"
    );

    // Delete the domain row — CASCADE should remove the listing_source_urls rows
    sqlx::query("DELETE FROM listing_source_domains WHERE domain_id = $1")
        .bind(domain_id)
        .execute(&pool)
        .await
        .unwrap();

    let count_after: (i64,) =
        sqlx::query_as("SELECT COUNT(*) FROM listing_source_urls WHERE domain_id = $1")
            .bind(domain_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        count_after.0, 0,
        "expected all listing_source_urls rows to be cascade-deleted when domain is deleted"
    );
}

// ---------------------------------------------------------------------------
// ON DELETE CASCADE — batch-inserted URLs are also removed on domain delete
// ---------------------------------------------------------------------------

#[serial]
#[aura_integration_test(services = [POSTGRES])]
async fn should_delete_batch_urls_when_domain_is_deleted() {
    let pool = get_postgres_client().await;
    let repository = UrlMetadataRepositoryImpl::new(pool.clone());

    let listing_source_id: ListingSourceId = uuid::Uuid::new_v4().into();
    let listing_source_id_uuid: uuid::Uuid = listing_source_id.into();
    let domain_id =
        insert_shop_with_domain(&pool, listing_source_id_uuid, "batch-cascade.example.com").await;

    let urls = vec![
        Url::parse("https://batch-cascade.example.com/p/1").unwrap(),
        Url::parse("https://batch-cascade.example.com/p/2").unwrap(),
        Url::parse("https://batch-cascade.example.com/p/3").unwrap(),
    ];
    let classes = vec![UrlClass::ProductListing; 3];
    repository
        .upsert_links_batch(&listing_source_id, &domain_id, &urls, &classes)
        .await
        .unwrap();

    let count_before: (i64,) =
        sqlx::query_as("SELECT COUNT(*) FROM listing_source_urls WHERE domain_id = $1")
            .bind(domain_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(count_before.0, 3);

    sqlx::query("DELETE FROM listing_source_domains WHERE domain_id = $1")
        .bind(domain_id)
        .execute(&pool)
        .await
        .unwrap();

    let count_after: (i64,) =
        sqlx::query_as("SELECT COUNT(*) FROM listing_source_urls WHERE domain_id = $1")
            .bind(domain_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        count_after.0, 0,
        "batch-inserted listing_source_urls should be cascade-deleted with the domain"
    );
}

// ---------------------------------------------------------------------------
// ON DELETE CASCADE — only URLs for the deleted domain are removed;
//                     URLs from a sibling domain on the same shop survive
// ---------------------------------------------------------------------------

#[serial]
#[aura_integration_test(services = [POSTGRES])]
async fn should_only_delete_urls_for_deleted_domain_not_sibling_domain() {
    let pool = get_postgres_client().await;
    let repository = UrlMetadataRepositoryImpl::new(pool.clone());

    let listing_source_id: ListingSourceId = uuid::Uuid::new_v4().into();
    let listing_source_id_uuid: uuid::Uuid = listing_source_id.into();

    // Two domains on the same shop
    let domain_id_to_delete =
        insert_shop_with_domain(&pool, listing_source_id_uuid, "delete-me.example.com").await;
    let domain_id_survivor =
        insert_domain_for_shop(&pool, listing_source_id_uuid, "keep-me.example.com").await;

    let url_a = Url::parse("https://delete-me.example.com/product/1").unwrap();
    let url_b = Url::parse("https://keep-me.example.com/product/1").unwrap();

    repository
        .upsert_link(
            &listing_source_id,
            &domain_id_to_delete,
            &url_a,
            &UrlClass::ProductListing,
        )
        .await
        .unwrap();
    repository
        .upsert_link(
            &listing_source_id,
            &domain_id_survivor,
            &url_b,
            &UrlClass::ProductListing,
        )
        .await
        .unwrap();

    // Delete only the first domain
    sqlx::query("DELETE FROM listing_source_domains WHERE domain_id = $1")
        .bind(domain_id_to_delete)
        .execute(&pool)
        .await
        .unwrap();

    let deleted_count: (i64,) =
        sqlx::query_as("SELECT COUNT(*) FROM listing_source_urls WHERE domain_id = $1")
            .bind(domain_id_to_delete)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        deleted_count.0, 0,
        "URLs for the deleted domain should be gone"
    );

    let survivor_count: (i64,) =
        sqlx::query_as("SELECT COUNT(*) FROM listing_source_urls WHERE domain_id = $1")
            .bind(domain_id_survivor)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        survivor_count.0, 1,
        "URLs for the surviving domain should remain"
    );
}

// ---------------------------------------------------------------------------
// upsert_links_batch — empty slice returns Ok(vec![])
// ---------------------------------------------------------------------------

#[serial]
#[aura_integration_test(services = [POSTGRES])]
async fn should_return_empty_vec_when_batch_is_empty() {
    let pool = get_postgres_client().await;
    let repository = UrlMetadataRepositoryImpl::new(pool.clone());

    let listing_source_id: ListingSourceId = uuid::Uuid::new_v4().into();
    let listing_source_id_uuid: uuid::Uuid = listing_source_id.into();
    let domain_id =
        insert_shop_with_domain(&pool, listing_source_id_uuid, "empty-batch.example.com").await;

    let result = repository
        .upsert_links_batch(&listing_source_id, &domain_id, &[], &[])
        .await
        .unwrap();

    assert!(result.is_empty(), "empty batch should return empty vec");
}

use super::*;
use party_core::party_id::PartyId;
use test_api::{IntegrationTestService, Postgres, aura_integration_test, get_postgres_client};
const SCHEMA: Postgres = Postgres::new("migrations");
async fn seed(pool: &sqlx::PgPool) -> (PartyId, ListingSourceId, UserId) {
    let owner = PartyId::new();
    let source = ListingSourceId::new();
    let user = UserId::new();
    let partnership = uuid::Uuid::now_v7();
    sqlx::query("INSERT INTO users(user_id,email,tier,role) VALUES($1,$2,'FREE','USER')")
        .bind(user.into_uuid())
        .bind(format!("{user}@example.test"))
        .execute(pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO parties(party_id,party_slug_id,name) VALUES($1,$2,'MCP Gallery')")
        .bind(owner.into_uuid())
        .bind(format!("gallery-{}", owner.as_uuid().simple()))
        .execute(pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO listing_sources(listing_source_id,listing_source_slug_id,name,operator_party_id) VALUES($1,$2,'MCP Gallery',$3)").bind(source.into_uuid()).bind(format!("source-{}", source.as_uuid().simple())).bind(owner.into_uuid()).execute(pool).await.unwrap();
    sqlx::query("INSERT INTO partnerships(partnership_id,party_id) VALUES($1,$2)")
        .bind(partnership)
        .bind(owner.into_uuid())
        .execute(pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO partnership_members(partnership_id,user_id) VALUES($1,$2)")
        .bind(partnership)
        .bind(user.into_uuid())
        .execute(pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO partnership_listing_source_grants(partnership_id,listing_source_id) VALUES($1,$2)").bind(partnership).bind(source.into_uuid()).execute(pool).await.unwrap();
    (owner, source, user)
}
#[aura_integration_test(services=[SCHEMA],isolated=true)]
async fn should_share_shipping_revision_creation_retry_lifecycle_evidence_and_paging_behavior() {
    let pool = get_postgres_client().await;
    let (_, source, user) = seed(&pool).await;
    let runtime = Runtime::from_pool(pool.clone());
    let registry = registry(&runtime);
    let ctx = both(user);
    let first = invoke(
        &registry,
        "createShippingProfile",
        creation(source, "first"),
        &ctx,
    )
    .await;
    assert_eq!(
        first,
        invoke(
            &registry,
            "createShippingProfile",
            creation(source, "first"),
            &ctx
        )
        .await
    );
    let id = first["shippingProfileId"].clone();
    let mut conflict = creation(source, "first");
    conflict["content"] = content(9500);
    assert_error(&registry, "createShippingProfile", conflict, &ctx, |e| {
        matches!(e, OperationError::Conflict(None))
    })
    .await;
    let details = json!({"listingSourceId":source,"shippingProfileId":id});
    assert_eq!(
        first,
        invoke(&registry, "getShippingProfile", details.clone(), &ctx).await
    );
    // Evidence changes advance management independently of semantic terms revisions.
    let replacement = json!({"listingSourceId":source,"shippingProfileId":id,"expectedRevision":1,"content":content(8000),"evidence":[{"reference":"https://example.test/new-evidence","assertion":"CORRECTED"}]});
    let updated = invoke(
        &registry,
        "replaceShippingProfile",
        replacement.clone(),
        &ctx,
    )
    .await;
    assert_eq!(json!(2), updated["managementRevision"]);
    assert_eq!(json!(1), updated["termsRevision"]);
    assert_error(
        &registry,
        "replaceShippingProfile",
        replacement,
        &ctx,
        |e| matches!(e, OperationError::Conflict(None)),
    )
    .await;
    let observed:String=sqlx::query_scalar("SELECT evidence->0->>'reference' FROM shipping_profile_evidence WHERE shipping_profile_id=$1 AND management_revision=2").bind(id.as_str().unwrap().parse::<ShippingProfileId>().unwrap().into_uuid()).fetch_one(&pool).await.unwrap();
    assert_eq!("https://example.test/new-evidence", observed);
    let default = invoke(
        &registry,
        "getShippingDefault",
        json!({"listingSourceId":source}),
        &ctx,
    )
    .await;
    assert!(default["shippingProfileId"].is_null());
    let assigned=invoke(&registry,"assignShippingDefault",json!({"listingSourceId":source,"shippingProfileId":id,"expectedRevision":default["revision"]}),&ctx).await;
    assert_eq!(json!("ACTIVE"), assigned["eligibility"]);
    let retired = invoke(
        &registry,
        "retireShippingProfile",
        json!({"listingSourceId":source,"shippingProfileId":id,"expectedRevision":2}),
        &ctx,
    )
    .await;
    assert_eq!(json!("RETIRED"), retired["lifecycle"]);
    assert_eq!(
        json!("RETIRED"),
        invoke(
            &registry,
            "getShippingDefault",
            json!({"listingSourceId":source}),
            &ctx
        )
        .await["eligibility"]
    );
    assert_error(&registry,"assignShippingDefault",json!({"listingSourceId":source,"shippingProfileId":id,"expectedRevision":assigned["revision"]}),&ctx,|e|matches!(e,OperationError::Conflict(None))).await;
    let restored=invoke(&registry,"restoreShippingProfile",json!({"listingSourceId":source,"shippingProfileId":id,"expectedRevision":retired["managementRevision"]}),&ctx).await;
    assert_eq!(json!("ACTIVE"), restored["lifecycle"]);
    let cleared = invoke(
        &registry,
        "clearShippingDefault",
        json!({"listingSourceId":source,"expectedRevision":assigned["revision"]}),
        &ctx,
    )
    .await;
    assert!(cleared["shippingProfileId"].is_null());
    let second = invoke(
        &registry,
        "createShippingProfile",
        creation(source, "second"),
        &ctx,
    )
    .await;
    let page = invoke(
        &registry,
        "listShippingProfiles",
        json!({"listingSourceId":source,"size":1}),
        &ctx,
    )
    .await;
    assert_eq!(1, page["result"].as_array().unwrap().len());
    let next = invoke(
        &registry,
        "listShippingProfiles",
        json!({"listingSourceId":source,"after":page["result"][0]["shippingProfileId"],"size":100}),
        &ctx,
    )
    .await;
    assert_eq!(1, next["result"].as_array().unwrap().len());
    assert_ne!(
        page["result"][0]["shippingProfileId"],
        next["result"][0]["shippingProfileId"]
    );
    assert_eq!(json!(1), second["termsRevision"]);
}
#[aura_integration_test(services=[SCHEMA],isolated=true)]
async fn should_recheck_delegated_scopes_operator_grants_account_status_and_attributed_party() {
    let pool = get_postgres_client().await;
    let (_, source, user) = seed(&pool).await;
    let (_, foreign, _) = seed(&pool).await;
    let runtime = Runtime::from_pool(pool.clone());
    let registry = registry(&runtime);
    let ctx = both(user);
    assert_error(
        &registry,
        "createShippingProfile",
        creation(foreign, "foreign"),
        &ctx,
        |e| matches!(e, OperationError::Forbidden),
    )
    .await;
    let mut attributed = creation(source, "unknown-party");
    attributed["content"]["attributedPartyId"] = json!(PartyId::new());
    assert_error(&registry, "createShippingProfile", attributed, &ctx, |e| {
        matches!(e, OperationError::NotFound)
    })
    .await;
    assert_error(
        &registry,
        "createShippingProfile",
        creation(source, "read-only"),
        &context(user, [CredentialCapability::ListingSourcesRead]),
        |e| matches!(e, OperationError::Forbidden),
    )
    .await;
    let result = invoke(
        &registry,
        "createShippingProfile",
        creation(source, "authorized"),
        &ctx,
    )
    .await;
    let input = json!({"listingSourceId":source,"shippingProfileId":result["shippingProfileId"]});
    assert_error(
        &registry,
        "getShippingProfile",
        input.clone(),
        &context(user, [CredentialCapability::ListingSourcesWrite]),
        |e| matches!(e, OperationError::Forbidden),
    )
    .await;
    let mut anonymous = ctx.clone();
    anonymous.operation.principal = Principal::Anonymous;
    assert_error(
        &registry,
        "getShippingProfile",
        input.clone(),
        &anonymous,
        |e| matches!(e, OperationError::Forbidden),
    )
    .await;
    sqlx::query("DELETE FROM partnership_listing_source_grants WHERE listing_source_id=$1")
        .bind(source.into_uuid())
        .execute(&pool)
        .await
        .unwrap();
    assert_error(&registry, "getShippingProfile", input.clone(), &ctx, |e| {
        matches!(e, OperationError::Forbidden)
    })
    .await;
    sqlx::query("UPDATE users SET role='ADMIN' WHERE user_id=$1")
        .bind(user.into_uuid())
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        result,
        invoke(&registry, "getShippingProfile", input.clone(), &ctx).await
    );
    sqlx::query("UPDATE users SET suspended=true WHERE user_id=$1")
        .bind(user.into_uuid())
        .execute(&pool)
        .await
        .unwrap();
    assert_error(&registry, "getShippingProfile", input, &ctx, |e| {
        matches!(e, OperationError::Forbidden)
    })
    .await;
}
#[aura_integration_test(services=[SCHEMA],isolated=true)]
async fn should_preserve_import_identity_exact_replay_provenance_and_current_authority() {
    let pool = get_postgres_client().await;
    let (_, source, user) = seed(&pool).await;
    let runtime = Runtime::from_pool(pool.clone());
    let registry = registry(&runtime);
    let ctx = both(user);
    // Acquire after the source/membership authority boundary.
    let acquired: time::OffsetDateTime = sqlx::query_scalar("SELECT clock_timestamp()")
        .fetch_one(&pool)
        .await
        .unwrap();
    let mut input = creation(source, "import-create");
    input["import"] = json!({"integration":{"kind":"SOURCE"},"sequence":1,"acquiredAt":acquired.format(&time::format_description::well_known::Rfc3339).unwrap(),"requestKey":"request-1","objectKey":"policy-1","observation":{}});
    let result = invoke(&registry, "createShippingProfile", input.clone(), &ctx).await;
    assert_eq!(
        result,
        invoke(&registry, "createShippingProfile", input.clone(), &ctx).await
    );
    let found = invoke(
        &registry,
        "listShippingProfiles",
        json!({"listingSourceId":source,"objectKey":"policy-1","size":100}),
        &ctx,
    )
    .await;
    assert_eq!(
        result["shippingProfileId"],
        found["result"][0]["shippingProfileId"]
    );
    let mut changed = input.clone();
    changed["content"] = content(9500);
    assert_error(&registry, "createShippingProfile", changed, &ctx, |e| {
        matches!(e, OperationError::Conflict(None))
    })
    .await;
    assert_error(
        &registry,
        "listShippingProfiles",
        json!({"listingSourceId":source,"integrationMethod":"SHOPIFY"}),
        &ctx,
        |e| matches!(e, OperationError::InvalidArguments(_)),
    )
    .await;
    sqlx::query("DELETE FROM partnership_listing_source_grants WHERE listing_source_id=$1")
        .bind(source.into_uuid())
        .execute(&pool)
        .await
        .unwrap();
    assert_error(&registry, "createShippingProfile", input, &ctx, |e| {
        matches!(e, OperationError::Forbidden)
    })
    .await;
}

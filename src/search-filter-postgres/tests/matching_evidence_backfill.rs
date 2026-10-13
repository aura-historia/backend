use test_api::{IntegrationTestService, Postgres, aura_integration_test, get_postgres_client};

const BUSINESS_SCHEMA: Postgres = Postgres::new("migrations");
const MIGRATION: &str =
    include_str!("../../../migrations/20261013120000_search_filter_matching_evidence.sql");

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn backfill_resolves_policies_and_quotas_preserves_authored_intent_and_fences_versions() {
    let pool = get_postgres_client().await;
    let mut tx = pool.begin().await.unwrap();
    // Reconstruct the prior schema inside a rolled-back transaction only.
    sqlx::raw_sql("ALTER TABLE search_filters DROP CONSTRAINT search_filters_active_enhanced_matching_check; ALTER TABLE search_filters DROP COLUMN matching_evidence_policy; DROP INDEX search_filters_periodic_match_eligible_idx; CREATE INDEX search_filters_periodic_match_eligible_idx ON search_filters (user_search_filter_id) WHERE state='ACTIVE' AND enhanced_search_description IS NOT NULL;")
        .execute(&mut *tx).await.unwrap();
    let mut ids = Vec::new();
    for (tier, count) in [("FREE", 3), ("PRO", 7), ("ULTIMATE", 2)] {
        let user = uuid::Uuid::now_v7();
        sqlx::query("INSERT INTO users (user_id,email,tier,role) VALUES ($1,$2,$3,'USER')")
            .bind(user)
            .bind(format!("{user}@example.test"))
            .bind(tier)
            .execute(&mut *tx)
            .await
            .unwrap();
        for index in 0..count {
            let id = uuid::Uuid::now_v7();
            let enhanced = (index == count - 1).then_some("Prefer blue antiques");
            sqlx::query("INSERT INTO search_filters (user_search_filter_id,user_id,name,state,search,enhanced_search_description,embedding,language,currency,created) VALUES ($1,$2,'backfill','ACTIVE',$3,$4,$5,'en','EUR', $6)")
                .bind(id).bind(user).bind(serde_json::json!({"enhanced_search_description": enhanced}))
                .bind(enhanced).bind(enhanced.map(|_| vec![1.0_f32; 768]))
                .bind(time::OffsetDateTime::UNIX_EPOCH + time::Duration::seconds(index.into()))
                .execute(&mut *tx).await.unwrap();
            ids.push((id, tier, index, count));
        }
    }
    sqlx::raw_sql(MIGRATION).execute(&mut *tx).await.unwrap();
    for (id, tier, index, count) in ids {
        let (policy, state, version, enhanced): (String, String, i64, Option<String>) = sqlx::query_as("SELECT matching_evidence_policy,state,version,enhanced_search_description FROM search_filters WHERE user_search_filter_id=$1")
            .bind(id).fetch_one(&mut *tx).await.unwrap();
        let expected_policy = match tier {
            "FREE" => "TEXT_ONLY",
            "PRO" => "TEXT_AND_ONE_IMAGE",
            _ => "ENHANCED_TEXT_AND_FOUR_IMAGES",
        };
        assert_eq!(expected_policy, policy);
        assert_eq!(2, version);
        let allowed = match tier {
            "FREE" => index == count - 2,
            "PRO" => index >= count - 6 && index < count - 1,
            _ => true,
        };
        assert_eq!(
            if allowed {
                "ACTIVE"
            } else {
                "INACTIVE_BY_RESTRICTED_PLAN"
            },
            state
        );
        assert_eq!(
            (index == count - 1).then(|| "Prefer blue antiques".to_owned()),
            enhanced
        );
    }
    tx.rollback().await.unwrap();
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn migration_rejects_missing_required_enhanced_embeddings_without_mutating_business_state() {
    let pool = get_postgres_client().await;
    let mut tx = pool.begin().await.unwrap();
    sqlx::raw_sql("ALTER TABLE search_filters DROP CONSTRAINT search_filters_active_enhanced_matching_check; ALTER TABLE search_filters DROP COLUMN matching_evidence_policy;")
        .execute(&mut *tx)
        .await
        .unwrap();
    let user = uuid::Uuid::now_v7();
    sqlx::query("INSERT INTO users (user_id,email,tier,role) VALUES ($1,$2,'ULTIMATE','USER')")
        .bind(user)
        .bind(format!("{user}@example.test"))
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::query("INSERT INTO search_filters (user_search_filter_id,user_id,name,state,search,enhanced_search_description,language,currency) VALUES ($1,$2,'missing embedding','ACTIVE','{}','blue antiques','en','EUR')")
        .bind(uuid::Uuid::now_v7()).bind(user).execute(&mut *tx).await.unwrap();
    assert!(sqlx::raw_sql(MIGRATION).execute(&mut *tx).await.is_err());
    tx.rollback().await.unwrap();
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM users WHERE user_id=$1")
        .bind(user)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(0, count);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn persisted_activation_requires_enhanced_capability_and_write_time_embedding() {
    let pool = get_postgres_client().await;
    let user = uuid::Uuid::now_v7();
    let filter = uuid::Uuid::now_v7();
    sqlx::query("INSERT INTO users (user_id,email,tier,role) VALUES ($1,$2,'ULTIMATE','USER')")
        .bind(user)
        .bind(format!("{user}@example.test"))
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO search_filters (user_search_filter_id,user_id,name,state,search,enhanced_search_description,matching_evidence_policy,language,currency) VALUES ($1,$2,'inactive intent','INACTIVE_BY_USER',$3,'blue antiques','TEXT_ONLY','en','EUR')")
        .bind(filter).bind(user).bind(serde_json::json!({"enhanced_search_description": "blue antiques"})).execute(&pool).await.unwrap();
    assert!(
        sqlx::query("UPDATE search_filters SET state='ACTIVE' WHERE user_search_filter_id=$1")
            .bind(filter)
            .execute(&pool)
            .await
            .is_err()
    );
    assert!(sqlx::query("UPDATE search_filters SET state='ACTIVE', matching_evidence_policy='ENHANCED_TEXT_AND_FOUR_IMAGES' WHERE user_search_filter_id=$1")
        .bind(filter).execute(&pool).await.is_err());
    sqlx::query("UPDATE search_filters SET state='ACTIVE', matching_evidence_policy='ENHANCED_TEXT_AND_FOUR_IMAGES', embedding=$2 WHERE user_search_filter_id=$1")
        .bind(filter).bind(vec![1.0_f32; 768]).execute(&pool).await.unwrap();
    let state: String =
        sqlx::query_scalar("SELECT state FROM search_filters WHERE user_search_filter_id=$1")
            .bind(filter)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!("ACTIVE", state);
}

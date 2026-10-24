use test_api::{IntegrationTestService, Postgres, aura_integration_test, get_postgres_client};

const BUSINESS_SCHEMA: Postgres = Postgres::new("migrations");

#[aura_integration_test(services = [BUSINESS_SCHEMA], isolated = true)]
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
    for statement in [
        "UPDATE search_filters SET state='ACTIVE' WHERE user_search_filter_id=$1",
        "UPDATE search_filters SET state='ACTIVE', matching_evidence_policy='ENHANCED_TEXT_AND_FOUR_IMAGES' WHERE user_search_filter_id=$1",
    ] {
        let error = sqlx::query(statement)
            .bind(filter)
            .execute(&pool)
            .await
            .unwrap_err();
        let database_error = error
            .as_database_error()
            .expect("database constraint failure");
        assert_eq!(Some("23514"), database_error.code().as_deref());
        assert_eq!(
            Some("search_filters_active_enhanced_matching_check"),
            database_error.constraint()
        );
    }
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

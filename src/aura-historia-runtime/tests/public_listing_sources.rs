use application::operation_context::{CorrelationId, OperationContext, Principal, RequestId};
use aura_historia_runtime::Runtime;
use listing_source_core::ListingSourceSlugId;
use listing_source_service::use_cases::queries::{
    get_public_listing_source_by_slug::{
        GetPublicListingSourceBySlugError, GetPublicListingSourceBySlugRequest,
    },
    search_public_listing_sources::{
        PublicListingSourceSearchQuery, SearchPublicListingSourcesRequest,
    },
};
use std::sync::Arc;
use test_api::{IntegrationTestService, Postgres, aura_integration_test, get_postgres_client};

const BUSINESS_SCHEMA: Postgres = Postgres::new("migrations");

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_compose_public_search_and_slug_detail_from_the_same_business_pool() {
    let result: Result<(), Box<dyn std::error::Error + Send + Sync>> = async {
        let pool = get_postgres_client().await;
        let source_id = uuid::Uuid::now_v7();
        let party_id = uuid::Uuid::now_v7();
        sqlx::query("INSERT INTO parties (party_id, party_slug_id, name) VALUES ($1, 'runtime-operator', 'Runtime Operator')")
            .bind(party_id)
            .execute(&pool)
            .await?;
        sqlx::query("INSERT INTO listing_sources (listing_source_id, listing_source_slug_id, name, operator_party_id) VALUES ($1, 'runtime-source', 'Runtime Source', $2)")
            .bind(source_id)
            .bind(party_id)
            .execute(&pool)
            .await?;

        let runtime = Runtime::from_pool(pool);
        let rest = runtime.public_listing_sources().clone();
        let mcp = runtime.public_listing_sources();
        assert!(Arc::ptr_eq(&rest.search, &mcp.search));
        assert!(Arc::ptr_eq(&rest.get_by_slug, &mcp.get_by_slug));
        let context = OperationContext {
            principal: Principal::Anonymous,
            request_id: RequestId::new("runtime-integration"),
            correlation_id: CorrelationId::new("runtime-integration"),
        };
        let result = rest.search.execute(&context, SearchPublicListingSourcesRequest::new(
            PublicListingSourceSearchQuery::new(Some("runtime".to_owned()))?,
            21,
            None,
        )?).await?;
        assert_eq!(1, result.items.len());
        assert!(result.continuation.is_none());
        let detail = mcp.get_by_slug.execute(&context, GetPublicListingSourceBySlugRequest {
            slug_id: ListingSourceSlugId::raw("runtime-source")?,
        }).await?;
        assert_eq!(result.items[0], detail);
        assert_eq!(&source_id, detail.listing_source_id.as_uuid());
        assert_eq!("Runtime Operator", detail.operator.name.as_ref());

        let missing = mcp.get_by_slug.execute(&context, GetPublicListingSourceBySlugRequest {
            slug_id: ListingSourceSlugId::raw("missing-source")?,
        }).await;
        assert!(matches!(missing, Err(GetPublicListingSourceBySlugError::NotFound)));
        Ok(())
    }.await;
    assert!(result.is_ok(), "{result:?}");
}

use super::router;
use crate::auth::{AuthError, AuthMethod, RequestMetadata, TokenAuthenticator, TransportPrincipal};
use crate::state::SearchFiltersState;
use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use common::event_id::EventId;
use common::operation_context::OperationContext;
use common::pagination::cursor::{Cursor, CursoredResult};
use common::product_id::ProductId;
use common::resource_state::domain::ResourceState;

use common::user_id::UserId;
use common::user_search_filter_id::UserSearchFilterId;
use common::user_search_filter_name::UserSearchFilterName;
use product_core::product_search::ProductSearch;
use search_filter_service::ports::{SearchFilterMatchView, SearchFilterView};
use search_filter_service::use_cases::{
    CreateSearchFilterCommand, CreateSearchFilterError, CreateSearchFilterResult,
    CreateSearchFilterUseCase, DeleteOwnedSearchFilterCommand, DeleteOwnedSearchFilterError,
    DeleteOwnedSearchFilterResult, DeleteOwnedSearchFilterUseCase, GetOwnedSearchFilterError,
    GetOwnedSearchFilterRequest, GetOwnedSearchFilterResult, GetOwnedSearchFilterUseCase,
    ListOwnedSearchFiltersError, ListOwnedSearchFiltersRequest, ListOwnedSearchFiltersResult,
    ListOwnedSearchFiltersUseCase, ListSearchFilterMatchesError, ListSearchFilterMatchesRequest,
    ListSearchFilterMatchesResult, ListSearchFilterMatchesUseCase, UpdateOwnedSearchFilterCommand,
    UpdateOwnedSearchFilterError, UpdateOwnedSearchFilterResult, UpdateOwnedSearchFilterUseCase,
    UpdateSearchFilterMatchFeedbackCommand, UpdateSearchFilterMatchFeedbackError,
    UpdateSearchFilterMatchFeedbackResult, UpdateSearchFilterMatchFeedbackUseCase,
};
use std::sync::{Arc, Mutex, MutexGuard};
use time::OffsetDateTime;
use tower::ServiceExt;

struct FakeAuthenticator(UserId);

#[async_trait::async_trait]
impl TokenAuthenticator for FakeAuthenticator {
    async fn authenticate(
        &self,
        _bearer_token: &str,
        _metadata: &RequestMetadata,
    ) -> Result<TransportPrincipal, AuthError> {
        Ok(TransportPrincipal::User {
            user_id: self.0,
            auth_method: AuthMethod::CognitoJwt,
            capabilities: Default::default(),
        })
    }
}

struct ListUseCase {
    filter: SearchFilterView,
}
#[async_trait::async_trait]
impl ListOwnedSearchFiltersUseCase for ListUseCase {
    async fn execute(
        &self,
        _context: &OperationContext,
        _request: ListOwnedSearchFiltersRequest,
    ) -> Result<ListOwnedSearchFiltersResult, ListOwnedSearchFiltersError> {
        Ok(ListOwnedSearchFiltersResult {
            items: vec![self.filter.clone()],
        })
    }
}

struct CreateUseCase {
    filter: SearchFilterView,
}
#[async_trait::async_trait]
impl CreateSearchFilterUseCase for CreateUseCase {
    async fn execute(
        &self,
        _context: &OperationContext,
        _command: CreateSearchFilterCommand,
    ) -> Result<CreateSearchFilterResult, CreateSearchFilterError> {
        Ok(CreateSearchFilterResult {
            filter: self.filter.clone(),
        })
    }
}

struct GetUseCase {
    filter: SearchFilterView,
}
#[async_trait::async_trait]
impl GetOwnedSearchFilterUseCase for GetUseCase {
    async fn execute(
        &self,
        _context: &OperationContext,
        _request: GetOwnedSearchFilterRequest,
    ) -> Result<GetOwnedSearchFilterResult, GetOwnedSearchFilterError> {
        Ok(GetOwnedSearchFilterResult {
            filter: self.filter.clone(),
        })
    }
}

type UpdateCalls = Arc<Mutex<Vec<UpdateOwnedSearchFilterCommand>>>;

struct UpdateUseCase {
    filter: SearchFilterView,
    calls: UpdateCalls,
}
#[async_trait::async_trait]
impl UpdateOwnedSearchFilterUseCase for UpdateUseCase {
    async fn execute(
        &self,
        _context: &OperationContext,
        command: UpdateOwnedSearchFilterCommand,
    ) -> Result<UpdateOwnedSearchFilterResult, UpdateOwnedSearchFilterError> {
        lock(&self.calls).push(command);
        Ok(UpdateOwnedSearchFilterResult {
            filter: self.filter.clone(),
        })
    }
}

struct DeleteUseCase;
#[async_trait::async_trait]
impl DeleteOwnedSearchFilterUseCase for DeleteUseCase {
    async fn execute(
        &self,
        _context: &OperationContext,
        _command: DeleteOwnedSearchFilterCommand,
    ) -> Result<DeleteOwnedSearchFilterResult, DeleteOwnedSearchFilterError> {
        Ok(DeleteOwnedSearchFilterResult)
    }
}

struct MatchesUseCase {
    result: ListSearchFilterMatchesResult,
}
#[async_trait::async_trait]
impl ListSearchFilterMatchesUseCase for MatchesUseCase {
    async fn execute(
        &self,
        _context: &OperationContext,
        _request: ListSearchFilterMatchesRequest,
    ) -> Result<ListSearchFilterMatchesResult, ListSearchFilterMatchesError> {
        Ok(self.result.clone())
    }
}

struct FeedbackUseCase {
    result: UpdateSearchFilterMatchFeedbackResult,
}
#[async_trait::async_trait]
impl UpdateSearchFilterMatchFeedbackUseCase for FeedbackUseCase {
    async fn execute(
        &self,
        _context: &OperationContext,
        _command: UpdateSearchFilterMatchFeedbackCommand,
    ) -> Result<UpdateSearchFilterMatchFeedbackResult, UpdateSearchFilterMatchFeedbackError> {
        Ok(self.result.clone())
    }
}

fn app() -> (
    Router,
    UserSearchFilterId,
    common::shop_id::ShopId,
    common::shops_product_id::ShopsProductId,
) {
    let (router, filter_id, shop_id, shops_product_id, _) = app_with_update_calls();
    (router, filter_id, shop_id, shops_product_id)
}

fn app_with_update_calls() -> (
    Router,
    UserSearchFilterId,
    common::shop_id::ShopId,
    common::shops_product_id::ShopsProductId,
    UpdateCalls,
) {
    let user_id = UserId::new();
    let filter_id = UserSearchFilterId::new();
    let now = OffsetDateTime::UNIX_EPOCH;
    let search = ProductSearch::default();
    let filter = SearchFilterView {
        search_filter_id: filter_id,
        user_id,
        name: UserSearchFilterName::from("Daily"),
        notifications: true,
        state: ResourceState::Active,
        search: search.clone(),
        embedding: None,
        created: now,
        updated: now,
        last_hybrid_search_matched: now,
    };

    let shop_id = common::shop_id::ShopId::new();
    let shops_product_id = common::shops_product_id::ShopsProductId::from("legacy-product");
    let search_filter_match = SearchFilterMatchView {
        user_id,
        search_filter_id: filter_id,
        search_filter_name: Some(UserSearchFilterName::from("Daily")),
        product_id: ProductId::new(),
        origin_event_id: EventId::new(),
        enhanced_match_reason: None,
        feedback: Some(true),
        created: now,
        updated: now,
    };
    let update_calls = Arc::new(Mutex::new(Vec::new()));
    let state = SearchFiltersState {
        list_owned_search_filters: Arc::new(ListUseCase {
            filter: filter.clone(),
        }),
        create_search_filter: Arc::new(CreateUseCase {
            filter: filter.clone(),
        }),
        get_owned_search_filter: Arc::new(GetUseCase {
            filter: filter.clone(),
        }),
        update_owned_search_filter: Arc::new(UpdateUseCase {
            filter,
            calls: Arc::clone(&update_calls),
        }),
        delete_owned_search_filter: Arc::new(DeleteUseCase),
        list_search_filter_matches: Arc::new(MatchesUseCase {
            result: ListSearchFilterMatchesResult {
                matches: CursoredResult {
                    cursor: Cursor::default(),
                    items: vec![search_filter_match.clone()],
                    total: Some(1),
                },
            },
        }),
        update_search_filter_match_feedback: Arc::new(FeedbackUseCase {
            result: UpdateSearchFilterMatchFeedbackResult {
                search_filter_match,
            },
        }),
        authenticator: Arc::new(FakeAuthenticator(user_id)),
    };
    (
        router(state),
        filter_id,
        shop_id,
        shops_product_id,
        update_calls,
    )
}

fn lock<T>(value: &Mutex<T>) -> MutexGuard<'_, T> {
    match value.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

fn authenticated(request: Request<Body>) -> Request<Body> {
    let (parts, body) = request.into_parts();
    let mut request = Request::from_parts(parts, body);
    request.headers_mut().insert(
        header::AUTHORIZATION,
        header::HeaderValue::from_static("Bearer test"),
    );
    request
}

#[tokio::test]
async fn should_list_search_filters_with_legacy_collection_and_no_store()
-> Result<(), Box<dyn std::error::Error>> {
    let (app, _, _, _) = app();
    let response = app
        .oneshot(authenticated(
            Request::get("/api/v1/me/search-filters?sort=created&order=desc").body(Body::empty())?,
        ))
        .await?;
    assert_eq!(StatusCode::OK, response.status());
    assert_eq!("no-store", response.headers()[header::CACHE_CONTROL]);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX).await?;
    let data: serde_json::Value = serde_json::from_slice(&body)?;
    assert_eq!(1, data["size"]);
    assert_eq!("Daily", data["items"][0]["name"]);
    Ok(())
}

#[tokio::test]
async fn should_create_search_filter_with_location_and_content_language()
-> Result<(), Box<dyn std::error::Error>> {
    let (app, filter_id, _, _) = app();
    let response = app
        .oneshot(authenticated(
            Request::post("/api/v1/me/search-filters")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    r#"{"name":"Daily","search":{"language":"en","currency":"EUR"}}"#,
                ))?,
        ))
        .await?;
    assert_eq!(StatusCode::CREATED, response.status());
    assert_eq!(
        format!("/api/v1/me/search-filters/{filter_id}"),
        response.headers()[header::LOCATION]
    );
    assert_eq!("en", response.headers()[header::CONTENT_LANGUAGE]);
    assert_eq!(
        "Thu, 01 Jan 1970 00:00:00 GMT",
        response.headers()[header::LAST_MODIFIED]
    );
    Ok(())
}

#[tokio::test]
async fn should_get_search_filter_with_representation_headers()
-> Result<(), Box<dyn std::error::Error>> {
    let (app, filter_id, _, _) = app();
    let response = app
        .oneshot(authenticated(
            Request::get(format!("/api/v1/me/search-filters/{filter_id}")).body(Body::empty())?,
        ))
        .await?;
    assert_eq!(StatusCode::OK, response.status());
    assert_eq!("no-store", response.headers()[header::CACHE_CONTROL]);
    assert_eq!("en", response.headers()[header::CONTENT_LANGUAGE]);
    assert_eq!(
        "Thu, 01 Jan 1970 00:00:00 GMT",
        response.headers()[header::LAST_MODIFIED]
    );
    Ok(())
}

#[tokio::test]
async fn should_map_nested_patch_fields_without_a_controller_read()
-> Result<(), Box<dyn std::error::Error>> {
    let (app, filter_id, _, _, calls) = app_with_update_calls();
    let response = app
        .oneshot(authenticated(
            Request::patch(format!("/api/v1/me/search-filters/{filter_id}"))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    r#"{"notifications":false,"search":{"language":"de","productQuery":["cabinet"]}}"#,
                ))?,
        ))
        .await?;
    assert_eq!(StatusCode::OK, response.status());
    assert_eq!("no-store", response.headers()[header::CACHE_CONTROL]);
    assert_eq!("en", response.headers()[header::CONTENT_LANGUAGE]);
    assert_eq!(
        "Thu, 01 Jan 1970 00:00:00 GMT",
        response.headers()[header::LAST_MODIFIED]
    );
    let calls = lock(&calls);
    assert_eq!(1, calls.len());
    assert!(matches!(
        &calls[0].search.language,
        common::patch_field::PatchField::Set(common::language::domain::Language::De)
    ));
    assert!(matches!(
        &calls[0].search.currency,
        common::patch_field::PatchField::Unchanged
    ));
    assert!(matches!(
        &calls[0].search.product_query,
        common::patch_field::PatchField::Set(_)
    ));
    Ok(())
}

#[tokio::test]
async fn should_delete_search_filter() -> Result<(), Box<dyn std::error::Error>> {
    let (app, filter_id, _, _) = app();
    let response = app
        .oneshot(authenticated(
            Request::delete(format!("/api/v1/me/search-filters/{filter_id}")).body(Body::empty())?,
        ))
        .await?;
    assert_eq!(StatusCode::NO_CONTENT, response.status());
    Ok(())
}

#[tokio::test]
async fn should_list_search_filter_matches_with_cursor_and_no_store()
-> Result<(), Box<dyn std::error::Error>> {
    let (app, filter_id, _, _) = app();
    let response = app.oneshot(authenticated(Request::get(format!("/api/v1/me/search-filters/{filter_id}/matches?sort=created&order=asc&from=1970-01-01T00%3A00%3A00Z&size=200")).body(Body::empty())?)).await?;
    assert_eq!(StatusCode::OK, response.status());
    assert_eq!("no-store", response.headers()[header::CACHE_CONTROL]);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX).await?;
    let data: serde_json::Value = serde_json::from_slice(&body)?;
    assert_eq!(1, data["items"].as_array().map_or(0, Vec::len));
    Ok(())
}

#[tokio::test]
async fn should_patch_match_feedback() -> Result<(), Box<dyn std::error::Error>> {
    let (app, filter_id, shop_id, shops_product_id) = app();
    let response = app
        .oneshot(authenticated(
            Request::patch(format!(
                "/api/v1/me/search-filters/{filter_id}/matches/{shop_id}/{shops_product_id}"
            ))
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(r#"{"feedback":true}"#))?,
        ))
        .await?;
    assert_eq!(StatusCode::OK, response.status());
    assert_eq!("no-store", response.headers()[header::CACHE_CONTROL]);
    assert_eq!(
        "Thu, 01 Jan 1970 00:00:00 GMT",
        response.headers()[header::LAST_MODIFIED]
    );
    Ok(())
}

#[tokio::test]
async fn should_reject_missing_bearer_token_before_use_case()
-> Result<(), Box<dyn std::error::Error>> {
    let (app, _, _, _) = app();
    let response = app
        .oneshot(Request::get("/api/v1/me/search-filters").body(Body::empty())?)
        .await?;
    assert_eq!(StatusCode::UNAUTHORIZED, response.status());
    Ok(())
}

#[tokio::test]
async fn should_reject_invalid_search_filter_path() -> Result<(), Box<dyn std::error::Error>> {
    let (app, _, _, _) = app();
    let response = app
        .oneshot(authenticated(
            Request::get("/api/v1/me/search-filters/not-a-uuid").body(Body::empty())?,
        ))
        .await?;
    assert_eq!(StatusCode::BAD_REQUEST, response.status());
    Ok(())
}

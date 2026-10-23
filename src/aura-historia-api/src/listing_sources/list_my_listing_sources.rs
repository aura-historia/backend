use crate::{
    auth::protected_context,
    error::{ApiError, BAD_QUERY_PARAMETER_VALUE},
    state::ListingSourcesState,
};
use aura_historia_contracts::listing_sources::administered::{
    AdministeredListingSourcesPageData, AdministeredListingSourcesQuery,
};
use axum::{
    extract::{RawQuery, State},
    http::HeaderMap,
    response::{IntoResponse, Response},
};

pub async fn list_my_listing_sources(
    State(state): State<ListingSourcesState>,
    headers: HeaderMap,
    RawQuery(raw): RawQuery,
) -> Response {
    let (context, _) = match protected_context(state.authenticator.as_ref(), &headers).await {
        Ok(value) => value,
        Err(response) => return *response,
    };
    let request = crate::wire::from_query_str::<AdministeredListingSourcesQuery>(
        raw.as_deref().unwrap_or_default(),
    )
    .map_err(|_| ApiError::bad_request(BAD_QUERY_PARAMETER_VALUE).with_query_field("query"))
    .and_then(|query| query.into_request().map_err(ApiError::from));
    let request = match request {
        Ok(request) => request,
        Err(error) => return error.into_response(),
    };
    match state.list_administered.execute(&context, request).await {
        Ok(result) => axum::Json(AdministeredListingSourcesPageData::from(result)).into_response(),
        Err(error) => ApiError::from(error).into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::{
        AuthError, AuthMethod, RequestMetadata, TokenAuthenticator, TransportPrincipal,
    };
    use crate::state::ListingSourcesState;
    use application::operation_context::OperationContext;
    use axum::Router;
    use axum::body::Body;
    use axum::http::{Request, StatusCode, header};
    use axum::routing::get;
    use listing_source_core::{ListingSourceId, ListingSourceName, ListingSourceSlugId};
    use listing_source_service::use_cases::commands::create_listing_source::{
        CreateListingSourceCommand, CreateListingSourceError, CreateListingSourceResult,
        CreateListingSourceUseCase,
    };
    use listing_source_service::use_cases::commands::update_listing_source::{
        UpdateListingSourceCommand, UpdateListingSourceError, UpdateListingSourceResult,
        UpdateListingSourceUseCase,
    };
    use listing_source_service::use_cases::queries::get_listing_source::{
        GetListingSourceError, GetListingSourceRequest, GetListingSourceResult,
        GetListingSourceUseCase,
    };
    use listing_source_service::use_cases::queries::search_listing_sources::{
        SearchListingSourcesError, SearchListingSourcesRequest, SearchListingSourcesResult,
        SearchListingSourcesUseCase,
    };
    use partnership_service::ports::AdministeredListingSource;
    use partnership_service::use_cases::queries::list_administered_listing_sources::{
        ListAdministeredListingSourcesError, ListAdministeredListingSourcesRequest,
        ListAdministeredListingSourcesResult, ListAdministeredListingSourcesUseCase,
    };
    use std::sync::Arc;
    use tower::ServiceExt;
    use user_core::user_id::UserId;

    struct Authenticator(UserId);

    #[async_trait::async_trait]
    impl TokenAuthenticator for Authenticator {
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

    struct ListUseCase;

    #[async_trait::async_trait]
    impl ListAdministeredListingSourcesUseCase for ListUseCase {
        async fn execute(
            &self,
            context: &OperationContext,
            request: ListAdministeredListingSourcesRequest,
        ) -> Result<ListAdministeredListingSourcesResult, ListAdministeredListingSourcesError>
        {
            assert_eq!(2, request.size);
            assert!(request.after.is_none());
            Ok(ListAdministeredListingSourcesResult {
                next: None,
                items: vec![AdministeredListingSource {
                    listing_source_id: ListingSourceId::new(),
                    slug_id: ListingSourceSlugId::raw("source-name")
                        .unwrap_or_else(|error| panic!("valid test listing source slug: {error}")),
                    name: ListingSourceName::try_from(match context.principal {
                        application::operation_context::Principal::User(id) => id.to_string(),
                        _ => panic!("expected trusted user"),
                    })
                    .unwrap_or_else(|error| panic!("invalid test listing source name: {error}")),
                }],
            })
        }
    }

    struct UnusedUseCases;

    #[async_trait::async_trait]
    impl CreateListingSourceUseCase for UnusedUseCases {
        async fn execute(
            &self,
            _context: &OperationContext,
            _command: CreateListingSourceCommand,
        ) -> Result<CreateListingSourceResult, CreateListingSourceError> {
            Err(CreateListingSourceError::Forbidden)
        }
    }

    #[async_trait::async_trait]
    impl GetListingSourceUseCase for UnusedUseCases {
        async fn execute(
            &self,
            _context: &OperationContext,
            _request: GetListingSourceRequest,
        ) -> Result<GetListingSourceResult, GetListingSourceError> {
            Err(GetListingSourceError::Forbidden)
        }
    }

    #[async_trait::async_trait]
    impl UpdateListingSourceUseCase for UnusedUseCases {
        async fn execute(
            &self,
            _context: &OperationContext,
            _command: UpdateListingSourceCommand,
        ) -> Result<UpdateListingSourceResult, UpdateListingSourceError> {
            Err(UpdateListingSourceError::Forbidden)
        }
    }

    #[async_trait::async_trait]
    impl SearchListingSourcesUseCase for UnusedUseCases {
        async fn execute(
            &self,
            _context: &OperationContext,
            _request: SearchListingSourcesRequest,
        ) -> Result<SearchListingSourcesResult, SearchListingSourcesError> {
            Err(SearchListingSourcesError::Forbidden)
        }
    }

    #[tokio::test]
    async fn should_list_listing_sources_administered_by_authenticated_user()
    -> Result<(), Box<dyn std::error::Error>> {
        let user_id = UserId::new();
        let state = ListingSourcesState::new(
            Arc::new(UnusedUseCases),
            Arc::new(UnusedUseCases),
            Arc::new(UnusedUseCases),
            Arc::new(ListUseCase),
            Arc::new(UnusedUseCases),
            Arc::new(Authenticator(user_id)),
        );
        let app = Router::new()
            .route("/v1/me/listing-sources", get(list_my_listing_sources))
            .with_state(state);

        let response = app
            .oneshot(
                Request::get("/v1/me/listing-sources?size=2")
                    .header(header::AUTHORIZATION, "Bearer token")
                    .body(Body::empty())?,
            )
            .await?;

        assert_eq!(StatusCode::OK, response.status());
        let body = axum::body::to_bytes(response.into_body(), usize::MAX).await?;
        let body: serde_json::Value = serde_json::from_slice(&body)?;
        assert_eq!(user_id.to_string(), body["items"][0]["name"]);
        assert!(body["items"][0]["listingSourceId"].is_string());
        assert_eq!("source-name", body["items"][0]["listingSourceSlugId"]);
        Ok(())
    }
}

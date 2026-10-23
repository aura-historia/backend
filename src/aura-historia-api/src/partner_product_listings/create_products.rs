use super::batch_response::{BatchResponse, batch_input_error};
use super::types::{
    CreateProductListingData, mutation_input_error, parse_listing_source_id,
    parse_partner_product_batch, prepare_batch,
};
use crate::auth::protected_context;
use crate::state::PartnerProductListingsState;
use axum::extract::{Path, State};
use axum::http::HeaderMap;
#[cfg(test)]
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};

pub async fn create_products(
    State(state): State<PartnerProductListingsState>,
    headers: HeaderMap,
    Path(raw_listing_source_id): Path<String>,
    body: String,
) -> Response {
    let listing_source_id = match parse_listing_source_id(&raw_listing_source_id) {
        Ok(listing_source_id) => listing_source_id,
        Err(error) => return error.into_response(),
    };
    let (context, _) = match protected_context(state.authenticator.as_ref(), &headers).await {
        Ok(value) => value,
        Err(response) => return *response,
    };
    let products: Vec<CreateProductListingData> = match parse_partner_product_batch(&body) {
        Ok(products) => products,
        Err(error) => return error.into_response(),
    };

    let prepared = match prepare_batch(listing_source_id, products) {
        Ok(prepared) => prepared,
        Err(error) => return mutation_input_error(error).into_response(),
    };
    match state.batches.create(&context, prepared.commands).await {
        Ok(result) => {
            BatchResponse::from_result(listing_source_id, prepared.source_listing_ids, result)
                .into_response()
        }
        Err(error) => batch_input_error(error).into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::{
        AuthError, AuthMethod, RequestMetadata, TokenAuthenticator, TransportPrincipal,
    };
    use application::operation_context::{CredentialCapability, OperationContext};
    use aura_historia_contracts::partner_product_listings::MAX_PARTNER_PRODUCT_LISTING_BATCH_SIZE;
    use axum::Router;
    use axum::body::Body;
    use axum::http::{Request, header};
    use listing_source_core::ListingSourceId;
    use product_listing_core::product_listing_id::{ProductListingId, ProductListingKey};
    use product_listing_core::product_listing_slug_id::ProductListingSlugId;
    use product_listing_service::use_cases::commands::partner_product_listing_batches::PartnerProductListingBatchesHandler;
    use product_listing_service::use_cases::{
        CreateProductListingCommand, CreateProductListingError, CreateProductListingResult,
        CreateProductListingUseCase, UpdateProductListingCommand, UpdateProductListingError,
        UpdateProductListingResult, UpdateProductListingUseCase, UpsertProductListingCommand,
        UpsertProductListingError, UpsertProductListingResult, UpsertProductListingUseCase,
        WithdrawProductListingError, WithdrawProductListingResult, WithdrawProductListingUseCase,
    };
    use serde_json::{Value, json};
    use std::collections::BTreeSet;
    use std::sync::Arc;
    use tower::ServiceExt;
    use user_core::user_id::UserId;

    mockall::mock! {
        CreateUseCase {}
        #[async_trait::async_trait]
        impl CreateProductListingUseCase for CreateUseCase {
            async fn execute(&self, context: &OperationContext, command: CreateProductListingCommand) -> Result<CreateProductListingResult, CreateProductListingError>;
        }
    }

    mockall::mock! {
        UpdateUseCase {}
        #[async_trait::async_trait]
        impl UpdateProductListingUseCase for UpdateUseCase {
            async fn execute(&self, context: &OperationContext, product_listing_id: ProductListingId, command: UpdateProductListingCommand) -> Result<UpdateProductListingResult, UpdateProductListingError>;
            async fn execute_by_key(&self, context: &OperationContext, product_key: ProductListingKey, command: UpdateProductListingCommand) -> Result<UpdateProductListingResult, UpdateProductListingError>;
        }
    }

    mockall::mock! {
        UpsertUseCase {}
        #[async_trait::async_trait]
        impl UpsertProductListingUseCase for UpsertUseCase {
            async fn execute(&self, context: &OperationContext, command: UpsertProductListingCommand) -> Result<UpsertProductListingResult, UpsertProductListingError>;
        }
    }

    mockall::mock! {
        WithdrawUseCase {}
        #[async_trait::async_trait]
        impl WithdrawProductListingUseCase for WithdrawUseCase {
            async fn execute(&self, context: &OperationContext, product_listing_id: ProductListingId) -> Result<WithdrawProductListingResult, WithdrawProductListingError>;
            async fn execute_by_key(&self, context: &OperationContext, product_key: ProductListingKey) -> Result<WithdrawProductListingResult, WithdrawProductListingError>;
        }
    }

    mockall::mock! {
        Authenticator {}
        #[async_trait::async_trait]
        impl TokenAuthenticator for Authenticator {
            async fn authenticate(&self, bearer_token: &str, metadata: &RequestMetadata) -> Result<TransportPrincipal, AuthError>;
        }
    }

    #[tokio::test]
    async fn should_return_empty_failures_when_all_creates_succeed()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut create = MockCreateUseCase::new();
        create
            .expect_execute()
            .times(2)
            .returning(|_, _| Ok(created()));
        let app = app(create);
        let listing_source_id = ListingSourceId::new();

        let response = request(
            &app,
            &format!("/v1/listing-sources/{listing_source_id}/product-listings"),
            format!("[{},{}]", product("first"), product("second")),
            true,
        )
        .await?;

        assert_eq!(StatusCode::OK, response.status());
        assert_eq!(json!([]), body_json(response).await?);
        Ok(())
    }

    #[tokio::test]
    async fn should_create_without_availability_assertion_when_it_is_omitted_or_null()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut create = MockCreateUseCase::new();
        create
            .expect_execute()
            .times(2)
            .withf(|_, command| command.availability.is_none())
            .returning(|_, _| Ok(created()));
        let app = app(create);
        let listing_source_id = ListingSourceId::new();
        let omitted = product("omitted").replace(",\"availability\":\"AVAILABLE\"", "");
        let explicit_null = product("null").replace("\"AVAILABLE\"", "null");

        let response = request(
            &app,
            &format!("/v1/listing-sources/{listing_source_id}/product-listings"),
            format!("[{omitted},{explicit_null}]"),
            true,
        )
        .await?;

        assert_eq!(StatusCode::OK, response.status());
        assert_eq!(json!([]), body_json(response).await?);
        Ok(())
    }

    #[tokio::test]
    async fn should_return_failed_key_when_create_batch_partially_succeeds()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut create = MockCreateUseCase::new();
        create.expect_execute().times(2).returning(|_, command| {
            if command.source_listing_id.as_ref() == "failed" {
                Err(CreateProductListingError::SourceListingAlreadyExists)
            } else {
                Ok(created())
            }
        });
        let app = app(create);
        let listing_source_id = ListingSourceId::new();

        let response = request(
            &app,
            &format!("/v1/listing-sources/{listing_source_id}/product-listings"),
            format!("[{},{}]", product("created"), product("failed")),
            true,
        )
        .await?;

        assert_eq!(StatusCode::OK, response.status());
        assert_eq!(
            json!([{
                "listingSourceId": listing_source_id.to_string(),
                "sourceListingId": "failed",
                "error": "CONFLICT"
            }]),
            body_json(response).await?
        );
        Ok(())
    }

    #[tokio::test]
    async fn should_return_first_error_when_all_creates_fail()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut create = MockCreateUseCase::new();
        create
            .expect_execute()
            .times(1)
            .returning(|_, _| Err(CreateProductListingError::SourceListingAlreadyExists));
        let app = app(create);
        let listing_source_id = ListingSourceId::new();

        let response = request(
            &app,
            &format!("/v1/listing-sources/{listing_source_id}/product-listings"),
            format!("[{}]", product("duplicate")),
            true,
        )
        .await?;

        assert_eq!(StatusCode::CONFLICT, response.status());
        assert_eq!("CONFLICT", body_json(response).await?["error"]);
        Ok(())
    }

    #[tokio::test]
    async fn should_reject_batch_larger_than_limit_before_create_use_case()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut create = MockCreateUseCase::new();
        create.expect_execute().never();
        let app = app(create);
        let listing_source_id = ListingSourceId::new();
        let body = format!(
            "[{}]",
            (0..=MAX_PARTNER_PRODUCT_LISTING_BATCH_SIZE)
                .map(|_| product("too-many"))
                .collect::<Vec<_>>()
                .join(",")
        );

        let response = request(
            &app,
            &format!("/v1/listing-sources/{listing_source_id}/product-listings"),
            body,
            true,
        )
        .await?;

        assert_eq!(StatusCode::BAD_REQUEST, response.status());
        assert_eq!(json!("BAD_BODY_VALUE"), body_json(response).await?["error"]);
        Ok(())
    }

    #[tokio::test]
    async fn should_reject_batch_with_title_without_text_before_create_use_case()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut create = MockCreateUseCase::new();
        create.expect_execute().never();
        let app = app(create);
        let listing_source_id = ListingSourceId::new();
        let emoji_only = product("emoji-only").replace("Cabinet", "🌿✨");

        let response = request(
            &app,
            &format!("/v1/listing-sources/{listing_source_id}/product-listings"),
            format!("[{},{emoji_only}]", product("created")),
            true,
        )
        .await?;

        assert_eq!(StatusCode::BAD_REQUEST, response.status());
        assert_eq!(json!("BAD_BODY_VALUE"), body_json(response).await?["error"]);
        Ok(())
    }

    #[tokio::test]
    async fn should_reject_missing_auth_or_invalid_body_before_create_use_case()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut create = MockCreateUseCase::new();
        create.expect_execute().never();
        let app = app(create);
        let listing_source_id = ListingSourceId::new();

        let missing_auth = request(
            &app,
            &format!("/v1/listing-sources/{listing_source_id}/product-listings"),
            format!("[{}]", product("created")),
            false,
        )
        .await?;
        assert_eq!(StatusCode::UNAUTHORIZED, missing_auth.status());

        let invalid_body = request(
            &app,
            &format!("/v1/listing-sources/{listing_source_id}/product-listings"),
            "{".to_owned(),
            true,
        )
        .await?;
        assert_eq!(StatusCode::BAD_REQUEST, invalid_body.status());
        Ok(())
    }

    #[tokio::test]
    async fn should_preserve_authenticated_empty_maximum_and_oversized_batch_behavior()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut use_case = MockCreateUseCase::new();
        use_case
            .expect_execute()
            .times(100)
            .returning(|_, _| Ok(created()));
        let app = app(use_case);
        let source = ListingSourceId::new();
        let path = format!("/v1/listing-sources/{source}/product-listings");
        assert_eq!(
            StatusCode::UNAUTHORIZED,
            request(&app, &path, "[]".to_owned(), false).await?.status()
        );
        let empty = request(&app, &path, "[]".to_owned(), true).await?;
        assert_eq!(StatusCode::OK, empty.status());
        assert_eq!(json!([]), body_json(empty).await?);
        let item = product("same");
        let oversized = format!("[{}]", vec![item.clone(); 101].join(","));
        assert_eq!(
            StatusCode::BAD_REQUEST,
            request(&app, &path, oversized, true).await?.status()
        );
        let maximum = format!("[{}]", vec![item; 100].join(","));
        let maximum = request(&app, &path, maximum, true).await?;
        assert_eq!(StatusCode::OK, maximum.status());
        assert_eq!(json!([]), body_json(maximum).await?);
        Ok(())
    }

    fn app(create: MockCreateUseCase) -> Router {
        let state = PartnerProductListingsState::new(
            Arc::new(PartnerProductListingBatchesHandler::new(
                Arc::new(create),
                Arc::new(MockUpdateUseCase::new()),
                Arc::new(MockUpsertUseCase::new()),
                Arc::new(MockWithdrawUseCase::new()),
            )),
            Arc::new(authenticator()),
        );
        Router::new()
            .route(
                "/v1/listing-sources/{listing_source_id}/product-listings",
                axum::routing::post(create_products),
            )
            .with_state(state)
    }

    fn authenticator() -> MockAuthenticator {
        let mut authenticator = MockAuthenticator::new();
        authenticator.expect_authenticate().returning(|_, _| {
            Ok(TransportPrincipal::User {
                user_id: UserId::new(),
                auth_method: AuthMethod::AuraAccessToken,
                capabilities: BTreeSet::from([CredentialCapability::ProductListingsWrite]),
            })
        });
        authenticator
    }

    fn created() -> CreateProductListingResult {
        CreateProductListingResult {
            product_listing_id: ProductListingId::new(),
            product_listing_title_slug_id: ProductListingSlugId::raw("created-product-a1b2c3")
                .unwrap_or_else(|error| panic!("valid product listing title slug: {error}")),
        }
    }

    async fn request(
        app: &Router,
        uri: &str,
        body: String,
        authenticated: bool,
    ) -> Result<Response, Box<dyn std::error::Error>> {
        let mut builder = Request::builder().method("POST").uri(uri);
        if authenticated {
            builder = builder.header(header::AUTHORIZATION, "Bearer valid");
        }
        Ok(app.clone().oneshot(builder.body(Body::from(body))?).await?)
    }

    async fn body_json(response: Response) -> Result<Value, Box<dyn std::error::Error>> {
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX).await?;
        Ok(serde_json::from_slice(&bytes)?)
    }

    fn product(source_listing_id: &str) -> String {
        format!(
            r#"{{"sourceListingId":"{source_listing_id}","title":{{"text":"Cabinet","language":"en"}},"description":{{"text":"Old cabinet","language":"en"}},"availability":"AVAILABLE","url":"https://source.example/product-listings/{source_listing_id}","images":[]}}"#
        )
    }
}

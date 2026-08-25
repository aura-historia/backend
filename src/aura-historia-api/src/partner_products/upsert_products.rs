use super::types::{
    PartnerProductFailureData, UpsertProductListingData, parse_partner_product_batch,
};
use crate::auth::protected_context;
use crate::error::{ApiError, INVALID_UUID};
use crate::state::PartnerProductsState;
use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use shop_core::shop_id::ShopId;

pub async fn upsert_products(
    State(state): State<PartnerProductsState>,
    headers: HeaderMap,
    Path(raw_shop_id): Path<String>,
    body: String,
) -> Response {
    let shop_id = match parse_shop_id(&raw_shop_id) {
        Ok(shop_id) => shop_id,
        Err(error) => return error.into_response(),
    };
    let (context, _) = match protected_context(state.authenticator.as_ref(), &headers).await {
        Ok(value) => value,
        Err(response) => return *response,
    };
    let products: Vec<UpsertProductListingData> = match parse_partner_product_batch(&body) {
        Ok(products) => products,
        Err(error) => return error.into_response(),
    };

    let mut failures = Vec::new();
    let mut first_error = None;
    let mut successes = 0;
    for product in products {
        let shop_listing_id = product.shop_listing_id.clone();
        match state
            .upsert
            .execute(&context, product.into_command(shop_id))
            .await
        {
            Ok(_) => successes += 1,
            Err(error) => {
                let error = ApiError::from(error);
                let error_code = error.code();
                first_error.get_or_insert(error);
                failures.push(PartnerProductFailureData::new(
                    shop_id,
                    shop_listing_id,
                    error_code,
                ));
            }
        }
    }

    if successes == 0
        && let Some(error) = first_error
    {
        return error.into_response();
    }

    (StatusCode::OK, Json(failures)).into_response()
}

fn parse_shop_id(value: &str) -> Result<ShopId, ApiError> {
    ShopId::try_from(value).map_err(|_| {
        ApiError::bad_request(INVALID_UUID)
            .with_path_field("shopId")
            .with_detail("Path parameter 'shopId' must be a UUID.")
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::{
        AuthError, AuthMethod, RequestMetadata, TokenAuthenticator, TransportPrincipal,
    };
    use application::operation_context::{CredentialCapability, OperationContext};
    use axum::Router;
    use axum::body::Body;
    use axum::http::{Request, header};
    use domain_primitives::event_id::EventId;
    use product_listing_core::product_listing_id::{ProductListingId, ProductListingKey};
    use product_listing_core::product_listing_slug_id::ProductListingSlugId;
    use product_listing_service::use_cases::{
        CreateProductListingCommand, CreateProductListingError, CreateProductListingResult,
        CreateProductListingUseCase, DeleteProductListingError, DeleteProductListingResult,
        DeleteProductListingUseCase, UpdateProductListingCommand, UpdateProductListingError,
        UpdateProductListingResult, UpdateProductListingUseCase, UpsertProductListingCommand,
        UpsertProductListingError, UpsertProductListingResult, UpsertProductListingUseCase,
    };
    use serde_json::{Value, json};
    use std::collections::BTreeSet;
    use std::sync::Arc;
    use tower::ServiceExt;
    use user_core::user_id::UserId;

    mockall::mock! { CreateUseCase {} #[async_trait::async_trait] impl CreateProductListingUseCase for CreateUseCase { async fn execute(&self, context: &OperationContext, command: CreateProductListingCommand) -> Result<CreateProductListingResult, CreateProductListingError>; } }
    mockall::mock! { UpdateUseCase {} #[async_trait::async_trait] impl UpdateProductListingUseCase for UpdateUseCase { async fn execute(&self, context: &OperationContext, product_id: ProductListingId, command: UpdateProductListingCommand) -> Result<UpdateProductListingResult, UpdateProductListingError>; async fn execute_by_key(&self, context: &OperationContext, product_key: ProductListingKey, command: UpdateProductListingCommand) -> Result<UpdateProductListingResult, UpdateProductListingError>; } }
    mockall::mock! { UpsertUseCase {} #[async_trait::async_trait] impl UpsertProductListingUseCase for UpsertUseCase { async fn execute(&self, context: &OperationContext, command: UpsertProductListingCommand) -> Result<UpsertProductListingResult, UpsertProductListingError>; } }
    mockall::mock! { DeleteUseCase {} #[async_trait::async_trait] impl DeleteProductListingUseCase for DeleteUseCase { async fn execute(&self, context: &OperationContext, product_id: ProductListingId) -> Result<DeleteProductListingResult, DeleteProductListingError>; async fn execute_by_key(&self, context: &OperationContext, product_key: ProductListingKey) -> Result<DeleteProductListingResult, DeleteProductListingError>; } }
    mockall::mock! { Authenticator {} #[async_trait::async_trait] impl TokenAuthenticator for Authenticator { async fn authenticate(&self, bearer_token: &str, metadata: &RequestMetadata) -> Result<TransportPrincipal, AuthError>; } }

    #[tokio::test]
    async fn should_upsert_each_batch_item() -> Result<(), Box<dyn std::error::Error>> {
        let mut upsert = MockUpsertUseCase::new();
        upsert
            .expect_execute()
            .times(2)
            .withf(|_, command| {
                command.shop_listing_id.as_ref() == "first"
                    || command.shop_listing_id.as_ref() == "second"
            })
            .returning(|_, _| Ok(created()));
        let app = app(upsert);
        let shop_id = ShopId::new();

        let response = request(
            &app,
            &format!("/api/v1/shops/{shop_id}/products"),
            r#"[{"shopsProductId":"first"},{"shopsProductId":"second"}]"#,
            true,
        )
        .await?;

        assert_eq!(StatusCode::OK, response.status());
        assert_eq!(json!([]), body_json(response).await?);
        Ok(())
    }

    #[tokio::test]
    async fn should_return_failed_key_when_upsert_batch_partially_succeeds()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut upsert = MockUpsertUseCase::new();
        upsert.expect_execute().times(2).returning(|_, command| {
            if command.shop_listing_id.as_ref() == "failed" {
                Err(UpsertProductListingError::InvalidProductState)
            } else {
                Ok(created())
            }
        });
        let app = app(upsert);
        let shop_id = ShopId::new();

        let response = request(
            &app,
            &format!("/api/v1/shops/{shop_id}/products"),
            r#"[{"shopsProductId":"created"},{"shopsProductId":"failed"}]"#,
            true,
        )
        .await?;

        assert_eq!(StatusCode::OK, response.status());
        assert_eq!(
            json!([{
                "shopId": shop_id.to_string(),
                "shopsProductId": "failed",
                "error": "BAD_BODY_VALUE"
            }]),
            body_json(response).await?
        );
        Ok(())
    }

    #[tokio::test]
    async fn should_map_all_invalid_upserts_to_bad_request()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut upsert = MockUpsertUseCase::new();
        upsert
            .expect_execute()
            .times(1)
            .returning(|_, _| Err(UpsertProductListingError::InvalidProductState));
        let app = app(upsert);
        let shop_id = ShopId::new();

        let response = request(
            &app,
            &format!("/api/v1/shops/{shop_id}/products"),
            r#"[{"shopsProductId":"invalid"}]"#,
            true,
        )
        .await?;

        assert_eq!(StatusCode::BAD_REQUEST, response.status());
        Ok(())
    }

    fn app(upsert: MockUpsertUseCase) -> Router {
        let state = PartnerProductsState::new(
            Arc::new(MockCreateUseCase::new()),
            Arc::new(MockUpdateUseCase::new()),
            Arc::new(upsert),
            Arc::new(MockDeleteUseCase::new()),
            Arc::new(authenticator()),
        );
        Router::new()
            .route(
                "/api/v1/shops/{shop_id}/products",
                axum::routing::put(upsert_products),
            )
            .with_state(state)
    }

    fn authenticator() -> MockAuthenticator {
        let mut authenticator = MockAuthenticator::new();
        authenticator.expect_authenticate().returning(|_, _| {
            Ok(TransportPrincipal::User {
                user_id: UserId::new(),
                auth_method: AuthMethod::AuraAccessToken,
                capabilities: BTreeSet::from([CredentialCapability::ProductsWrite]),
            })
        });
        authenticator
    }

    fn created() -> UpsertProductListingResult {
        UpsertProductListingResult::Created(CreateProductListingResult {
            product_id: ProductListingId::new(),
            product_slug_id: ProductListingSlugId::from("upserted-product"),
            event_id: EventId::new(),
        })
    }

    async fn request(
        app: &Router,
        uri: &str,
        body: &str,
        authenticated: bool,
    ) -> Result<Response, Box<dyn std::error::Error>> {
        let mut builder = Request::builder().method("PUT").uri(uri);
        if authenticated {
            builder = builder.header(header::AUTHORIZATION, "Bearer valid");
        }
        Ok(app
            .clone()
            .oneshot(builder.body(Body::from(body.to_owned()))?)
            .await?)
    }

    async fn body_json(response: Response) -> Result<Value, Box<dyn std::error::Error>> {
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX).await?;
        Ok(serde_json::from_slice(&bytes)?)
    }
}

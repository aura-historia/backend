use super::types::{PartnerProductFailureData, UpsertProductData, parse_partner_product_batch};
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
        Err(response) => return response,
    };
    let products: Vec<UpsertProductData> = match parse_partner_product_batch(&body) {
        Ok(products) => products,
        Err(error) => return error.into_response(),
    };

    let mut failures = Vec::new();
    let mut first_error = None;
    let mut successes = 0;
    for product in products {
        let shops_product_id = product.shops_product_id.clone();
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
                    shops_product_id,
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
    use product_core::product_id::{ProductId, ProductKey};
    use product_core::product_slug_id::ProductSlugId;
    use product_service::use_cases::{
        CreateProductCommand, CreateProductError, CreateProductResult, CreateProductUseCase,
        DeleteProductError, DeleteProductResult, DeleteProductUseCase, UpdateProductCommand,
        UpdateProductError, UpdateProductResult, UpdateProductUseCase, UpsertProductCommand,
        UpsertProductError, UpsertProductResult, UpsertProductUseCase,
    };
    use serde_json::{Value, json};
    use std::collections::BTreeSet;
    use std::sync::Arc;
    use tower::ServiceExt;
    use user_core::user_id::UserId;

    mockall::mock! { CreateUseCase {} #[async_trait::async_trait] impl CreateProductUseCase for CreateUseCase { async fn execute(&self, context: &OperationContext, command: CreateProductCommand) -> Result<CreateProductResult, CreateProductError>; } }
    mockall::mock! { UpdateUseCase {} #[async_trait::async_trait] impl UpdateProductUseCase for UpdateUseCase { async fn execute(&self, context: &OperationContext, product_id: ProductId, command: UpdateProductCommand) -> Result<UpdateProductResult, UpdateProductError>; async fn execute_by_key(&self, context: &OperationContext, product_key: ProductKey, command: UpdateProductCommand) -> Result<UpdateProductResult, UpdateProductError>; } }
    mockall::mock! { UpsertUseCase {} #[async_trait::async_trait] impl UpsertProductUseCase for UpsertUseCase { async fn execute(&self, context: &OperationContext, command: UpsertProductCommand) -> Result<UpsertProductResult, UpsertProductError>; } }
    mockall::mock! { DeleteUseCase {} #[async_trait::async_trait] impl DeleteProductUseCase for DeleteUseCase { async fn execute(&self, context: &OperationContext, product_id: ProductId) -> Result<DeleteProductResult, DeleteProductError>; async fn execute_by_key(&self, context: &OperationContext, product_key: ProductKey) -> Result<DeleteProductResult, DeleteProductError>; } }
    mockall::mock! { Authenticator {} #[async_trait::async_trait] impl TokenAuthenticator for Authenticator { async fn authenticate(&self, bearer_token: &str, metadata: &RequestMetadata) -> Result<TransportPrincipal, AuthError>; } }

    #[tokio::test]
    async fn should_upsert_each_batch_item() -> Result<(), Box<dyn std::error::Error>> {
        let mut upsert = MockUpsertUseCase::new();
        upsert
            .expect_execute()
            .times(2)
            .withf(|_, command| {
                command.shops_product_id.as_ref() == "first"
                    || command.shops_product_id.as_ref() == "second"
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
            if command.shops_product_id.as_ref() == "failed" {
                Err(UpsertProductError::InvalidProductState)
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
            .returning(|_, _| Err(UpsertProductError::InvalidProductState));
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

    fn created() -> UpsertProductResult {
        UpsertProductResult::Created(CreateProductResult {
            product_id: ProductId::new(),
            product_slug_id: ProductSlugId::from("upserted-product"),
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

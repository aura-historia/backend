use super::types::{PartnerProductFailureData, UpdateProductData, parse_partner_product_batch};
use crate::auth::protected_context;
use crate::error::{ApiError, INVALID_UUID};
use crate::state::PartnerProductsState;
use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use shop_core::shop_id::ShopId;

pub async fn update_products(
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
    let products: Vec<UpdateProductData> = match parse_partner_product_batch(&body) {
        Ok(products) => products,
        Err(error) => return error.into_response(),
    };

    let mapped = match products
        .into_iter()
        .map(|product| {
            let shops_product_id = product.shops_product_id.clone();
            product
                .into_key_and_command(shop_id)
                .map(|(product_key, command)| (shops_product_id, product_key, command))
        })
        .collect::<Result<Vec<_>, ApiError>>()
    {
        Ok(mapped) => mapped,
        Err(error) => return error.into_response(),
    };

    let mut failures = Vec::new();
    let mut first_error = None;
    let mut successes = 0;
    for (shops_product_id, product_key, command) in mapped {
        match state
            .update
            .execute_by_key(&context, product_key, command)
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
    async fn should_call_key_update_for_each_successful_batch_item()
    -> Result<(), Box<dyn std::error::Error>> {
        let shop_id = ShopId::new();
        let expected_shop_id = shop_id;
        let mut update = MockUpdateUseCase::new();
        update
            .expect_execute_by_key()
            .times(2)
            .withf(move |_, key, command| {
                key.shop_id == expected_shop_id
                    && (key.shops_product_id.as_ref() == "first"
                        || key.shops_product_id.as_ref() == "second")
                    && !command.is_empty()
            })
            .returning(|_, _, _| Ok(updated()));
        let app = app(update);

        let response = request(&app, &format!("/api/v1/shops/{shop_id}/products"), r#"[{"shopsProductId":"first","state":"AVAILABLE"},{"shopsProductId":"second","state":"SOLD"}]"#, true).await?;

        assert_eq!(StatusCode::OK, response.status());
        assert_eq!(json!([]), body_json(response).await?);
        Ok(())
    }

    #[tokio::test]
    async fn should_return_failed_key_when_update_batch_partially_succeeds()
    -> Result<(), Box<dyn std::error::Error>> {
        let shop_id = ShopId::new();
        let mut update = MockUpdateUseCase::new();
        update
            .expect_execute_by_key()
            .times(2)
            .returning(|_, key, _| {
                if key.shops_product_id.as_ref() == "missing" {
                    Err(UpdateProductError::ProductNotFound)
                } else {
                    Ok(updated())
                }
            });
        let app = app(update);

        let response = request(&app, &format!("/api/v1/shops/{shop_id}/products"), r#"[{"shopsProductId":"present","state":"AVAILABLE"},{"shopsProductId":"missing","state":"AVAILABLE"}]"#, true).await?;

        assert_eq!(StatusCode::OK, response.status());
        assert_eq!(
            json!([{
                "shopId": shop_id.to_string(),
                "shopsProductId": "missing",
                "error": "PRODUCT_NOT_FOUND"
            }]),
            body_json(response).await?
        );
        Ok(())
    }

    #[tokio::test]
    async fn should_map_all_missing_updates_to_not_found() -> Result<(), Box<dyn std::error::Error>>
    {
        let mut update = MockUpdateUseCase::new();
        update
            .expect_execute_by_key()
            .times(1)
            .returning(|_, _, _| Err(UpdateProductError::ProductNotFound));
        let app = app(update);
        let shop_id = ShopId::new();

        let response = request(
            &app,
            &format!("/api/v1/shops/{shop_id}/products"),
            r#"[{"shopsProductId":"missing","state":"AVAILABLE"}]"#,
            true,
        )
        .await?;

        assert_eq!(StatusCode::NOT_FOUND, response.status());
        assert_eq!("PRODUCT_NOT_FOUND", body_json(response).await?["error"]);
        Ok(())
    }

    #[tokio::test]
    async fn should_reject_null_for_non_nullable_batch_member_before_any_write()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut update = MockUpdateUseCase::new();
        update.expect_execute_by_key().never();
        let app = app(update);
        let shop_id = ShopId::new();

        let response = request(
            &app,
            &format!("/api/v1/shops/{shop_id}/products"),
            r#"[{"shopsProductId":"valid","state":"AVAILABLE"},{"shopsProductId":"invalid","state":null}]"#,
            true,
        )
        .await?;

        assert_eq!(StatusCode::BAD_REQUEST, response.status());
        assert_eq!("BAD_BODY_VALUE", body_json(response).await?["error"]);
        Ok(())
    }

    #[tokio::test]
    async fn should_reject_invalid_update_body_before_use_case()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut update = MockUpdateUseCase::new();
        update.expect_execute_by_key().never();
        let app = app(update);
        let shop_id = ShopId::new();

        let response = request(
            &app,
            &format!("/api/v1/shops/{shop_id}/products"),
            "{",
            true,
        )
        .await?;

        assert_eq!(StatusCode::BAD_REQUEST, response.status());
        Ok(())
    }

    fn app(update: MockUpdateUseCase) -> Router {
        let state = PartnerProductsState::new(
            Arc::new(MockCreateUseCase::new()),
            Arc::new(update),
            Arc::new(MockUpsertUseCase::new()),
            Arc::new(MockDeleteUseCase::new()),
            Arc::new(authenticator()),
        );
        Router::new()
            .route(
                "/api/v1/shops/{shop_id}/products",
                axum::routing::patch(update_products),
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

    fn updated() -> UpdateProductResult {
        UpdateProductResult {
            product_id: ProductId::new(),
            event_id: Some(EventId::new()),
        }
    }

    async fn request(
        app: &Router,
        uri: &str,
        body: &str,
        authenticated: bool,
    ) -> Result<Response, Box<dyn std::error::Error>> {
        let mut builder = Request::builder().method("PATCH").uri(uri);
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

use super::types::{
    PartnerProductFailureData, WithdrawProductListingData, parse_partner_product_batch,
};
use crate::auth::protected_context;
use crate::error::{ApiError, INVALID_UUID};
use crate::state::PartnerProductListingsState;
use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use shop_core::shop_id::ShopId;

pub async fn delete_products(
    State(state): State<PartnerProductListingsState>,
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
    let products: Vec<WithdrawProductListingData> = match parse_partner_product_batch(&body) {
        Ok(products) => products,
        Err(error) => return error.into_response(),
    };

    let mut failures = Vec::new();
    let mut first_error = None;
    let mut successes = 0;
    for product in products {
        let shop_listing_id = product.shop_listing_id.clone();
        match state
            .withdraw
            .execute_by_key(&context, product.into_product_key(shop_id))
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

    mockall::mock! { CreateUseCase {} #[async_trait::async_trait] impl CreateProductListingUseCase for CreateUseCase { async fn execute(&self, context: &OperationContext, command: CreateProductListingCommand) -> Result<CreateProductListingResult, CreateProductListingError>; } }
    mockall::mock! { UpdateUseCase {} #[async_trait::async_trait] impl UpdateProductListingUseCase for UpdateUseCase { async fn execute(&self, context: &OperationContext, product_listing_id: ProductListingId, command: UpdateProductListingCommand) -> Result<UpdateProductListingResult, UpdateProductListingError>; async fn execute_by_key(&self, context: &OperationContext, product_key: ProductListingKey, command: UpdateProductListingCommand) -> Result<UpdateProductListingResult, UpdateProductListingError>; } }
    mockall::mock! { UpsertUseCase {} #[async_trait::async_trait] impl UpsertProductListingUseCase for UpsertUseCase { async fn execute(&self, context: &OperationContext, command: UpsertProductListingCommand) -> Result<UpsertProductListingResult, UpsertProductListingError>; } }
    mockall::mock! { WithdrawUseCase {} #[async_trait::async_trait] impl WithdrawProductListingUseCase for WithdrawUseCase { async fn execute(&self, context: &OperationContext, product_listing_id: ProductListingId) -> Result<WithdrawProductListingResult, WithdrawProductListingError>; async fn execute_by_key(&self, context: &OperationContext, product_key: ProductListingKey) -> Result<WithdrawProductListingResult, WithdrawProductListingError>; } }
    mockall::mock! { Authenticator {} #[async_trait::async_trait] impl TokenAuthenticator for Authenticator { async fn authenticate(&self, bearer_token: &str, metadata: &RequestMetadata) -> Result<TransportPrincipal, AuthError>; } }

    #[tokio::test]
    async fn should_delete_each_collection_item_by_product_key()
    -> Result<(), Box<dyn std::error::Error>> {
        let shop_id = ShopId::new();
        let expected_shop_id = shop_id;
        let mut withdraw = MockWithdrawUseCase::new();
        withdraw
            .expect_execute_by_key()
            .times(2)
            .withf(move |_, key| {
                key.shop_id == expected_shop_id
                    && (key.shop_listing_id.as_ref() == "first"
                        || key.shop_listing_id.as_ref() == "second")
            })
            .returning(|_, _| Ok(withdrawn()));
        let app = app(withdraw);

        let response = request(
            &app,
            &format!("/api/v1/shops/{shop_id}/product-listings"),
            r#"[{"shopListingId":"first"},{"shopListingId":"second"}]"#,
            true,
        )
        .await?;

        assert_eq!(StatusCode::OK, response.status());
        assert_eq!(json!([]), body_json(response).await?);
        Ok(())
    }

    #[tokio::test]
    async fn should_return_failed_key_when_delete_batch_partially_succeeds()
    -> Result<(), Box<dyn std::error::Error>> {
        let shop_id = ShopId::new();
        let mut withdraw = MockWithdrawUseCase::new();
        withdraw
            .expect_execute_by_key()
            .times(2)
            .returning(|_, key| {
                if key.shop_listing_id.as_ref() == "missing" {
                    Err(WithdrawProductListingError::NotFound)
                } else {
                    Ok(withdrawn())
                }
            });
        let app = app(withdraw);

        let response = request(
            &app,
            &format!("/api/v1/shops/{shop_id}/product-listings"),
            r#"[{"shopListingId":"present"},{"shopListingId":"missing"}]"#,
            true,
        )
        .await?;

        assert_eq!(StatusCode::OK, response.status());
        assert_eq!(
            json!([{
                "shopId": shop_id.to_string(),
                "shopListingId": "missing",
                "error": "PRODUCT_LISTING_NOT_FOUND"
            }]),
            body_json(response).await?
        );
        Ok(())
    }

    #[tokio::test]
    async fn should_map_all_missing_deletes_to_not_found_and_leave_legacy_item_route_absent()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut withdraw = MockWithdrawUseCase::new();
        withdraw
            .expect_execute_by_key()
            .times(1)
            .returning(|_, _| Err(WithdrawProductListingError::NotFound));
        let app = app(withdraw);
        let shop_id = ShopId::new();

        let missing = request(
            &app,
            &format!("/api/v1/shops/{shop_id}/product-listings"),
            r#"[{"shopListingId":"missing"}]"#,
            true,
        )
        .await?;
        assert_eq!(StatusCode::NOT_FOUND, missing.status());
        assert_eq!(
            "PRODUCT_LISTING_NOT_FOUND",
            body_json(missing).await?["error"]
        );

        let old_route = request(
            &app,
            &format!("/api/v1/shops/{shop_id}/product-listings/missing"),
            "",
            true,
        )
        .await?;
        assert_eq!(StatusCode::NOT_FOUND, old_route.status());
        Ok(())
    }

    fn app(withdraw: MockWithdrawUseCase) -> Router {
        let state = PartnerProductListingsState::new(
            Arc::new(MockCreateUseCase::new()),
            Arc::new(MockUpdateUseCase::new()),
            Arc::new(MockUpsertUseCase::new()),
            Arc::new(withdraw),
            Arc::new(authenticator()),
        );
        Router::new()
            .route(
                "/api/v1/shops/{shop_id}/product-listings",
                axum::routing::delete(delete_products),
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

    fn withdrawn() -> WithdrawProductListingResult {
        WithdrawProductListingResult {
            product_listing_id: ProductListingId::new(),
            event_id: EventId::new(),
        }
    }

    async fn request(
        app: &Router,
        uri: &str,
        body: &str,
        authenticated: bool,
    ) -> Result<Response, Box<dyn std::error::Error>> {
        let mut builder = Request::builder().method("DELETE").uri(uri);
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

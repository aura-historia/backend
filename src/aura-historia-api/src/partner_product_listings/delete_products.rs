use super::batch_response::{BatchResponse, batch_input_error};
use super::types::{
    WithdrawProductListingData, mutation_input_error, parse_listing_source_id,
    parse_partner_product_batch, prepare_batch,
};
use crate::auth::protected_context;
use crate::state::PartnerProductListingsState;
use axum::extract::{Path, State};
use axum::http::HeaderMap;
#[cfg(test)]
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};

pub async fn delete_products(
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
    let products: Vec<WithdrawProductListingData> = match parse_partner_product_batch(&body) {
        Ok(products) => products,
        Err(error) => return error.into_response(),
    };

    let prepared = match prepare_batch(listing_source_id, products) {
        Ok(prepared) => prepared,
        Err(error) => return mutation_input_error(error).into_response(),
    };
    match state.batches.withdraw(&context, prepared.commands).await {
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
    use axum::Router;
    use axum::body::Body;
    use axum::http::{Request, header};
    use domain_primitives::change_outcome::ChangeOutcome;
    use listing_source_core::ListingSourceId;
    use product_listing_core::product_listing_id::{ProductListingId, ProductListingKey};
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

    mockall::mock! { CreateUseCase {} #[async_trait::async_trait] impl CreateProductListingUseCase for CreateUseCase { async fn execute(&self, context: &OperationContext, command: CreateProductListingCommand) -> Result<CreateProductListingResult, CreateProductListingError>; } }
    mockall::mock! { UpdateUseCase {} #[async_trait::async_trait] impl UpdateProductListingUseCase for UpdateUseCase { async fn execute(&self, context: &OperationContext, product_listing_id: ProductListingId, command: UpdateProductListingCommand) -> Result<UpdateProductListingResult, UpdateProductListingError>; async fn execute_by_key(&self, context: &OperationContext, product_key: ProductListingKey, command: UpdateProductListingCommand) -> Result<UpdateProductListingResult, UpdateProductListingError>; } }
    mockall::mock! { UpsertUseCase {} #[async_trait::async_trait] impl UpsertProductListingUseCase for UpsertUseCase { async fn execute(&self, context: &OperationContext, command: UpsertProductListingCommand) -> Result<UpsertProductListingResult, UpsertProductListingError>; } }
    mockall::mock! { WithdrawUseCase {} #[async_trait::async_trait] impl WithdrawProductListingUseCase for WithdrawUseCase { async fn execute(&self, context: &OperationContext, product_listing_id: ProductListingId) -> Result<WithdrawProductListingResult, WithdrawProductListingError>; async fn execute_by_key(&self, context: &OperationContext, product_key: ProductListingKey) -> Result<WithdrawProductListingResult, WithdrawProductListingError>; } }
    mockall::mock! { Authenticator {} #[async_trait::async_trait] impl TokenAuthenticator for Authenticator { async fn authenticate(&self, bearer_token: &str, metadata: &RequestMetadata) -> Result<TransportPrincipal, AuthError>; } }

    #[tokio::test]
    async fn should_delete_each_collection_item_by_product_key()
    -> Result<(), Box<dyn std::error::Error>> {
        let listing_source_id = ListingSourceId::new();
        let expected_listing_source_id = listing_source_id;
        let mut withdraw = MockWithdrawUseCase::new();
        withdraw
            .expect_execute_by_key()
            .times(2)
            .withf(move |_, key| {
                key.listing_source_id == expected_listing_source_id
                    && (key.source_listing_id.as_ref() == "first"
                        || key.source_listing_id.as_ref() == "second")
            })
            .returning(|_, _| Ok(withdrawn()));
        let app = app(withdraw);

        let response = request(
            &app,
            &format!("/v1/listing-sources/{listing_source_id}/product-listings"),
            r#"[{"sourceListingId":"first"},{"sourceListingId":"second"}]"#,
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
        let listing_source_id = ListingSourceId::new();
        let mut withdraw = MockWithdrawUseCase::new();
        withdraw
            .expect_execute_by_key()
            .times(2)
            .returning(|_, key| {
                if key.source_listing_id.as_ref() == "missing" {
                    Err(WithdrawProductListingError::NotFound)
                } else {
                    Ok(withdrawn())
                }
            });
        let app = app(withdraw);

        let response = request(
            &app,
            &format!("/v1/listing-sources/{listing_source_id}/product-listings"),
            r#"[{"sourceListingId":"present"},{"sourceListingId":"missing"}]"#,
            true,
        )
        .await?;

        assert_eq!(StatusCode::OK, response.status());
        assert_eq!(
            json!([{
                "listingSourceId": listing_source_id.to_string(),
                "sourceListingId": "missing",
                "error": "PRODUCT_LISTING_NOT_FOUND"
            }]),
            body_json(response).await?
        );
        Ok(())
    }

    #[tokio::test]
    async fn should_map_all_missing_deletes_to_not_found_and_leave_legacy_shop_route_absent()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut withdraw = MockWithdrawUseCase::new();
        withdraw
            .expect_execute_by_key()
            .times(1)
            .returning(|_, _| Err(WithdrawProductListingError::NotFound));
        let app = app(withdraw);
        let listing_source_id = ListingSourceId::new();

        let missing = request(
            &app,
            &format!("/v1/listing-sources/{listing_source_id}/product-listings"),
            r#"[{"sourceListingId":"missing"}]"#,
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
            &format!("/v1/shops/{listing_source_id}/product-listings"),
            "",
            true,
        )
        .await?;
        assert_eq!(StatusCode::NOT_FOUND, old_route.status());
        Ok(())
    }

    #[tokio::test]
    async fn should_preserve_authenticated_empty_maximum_and_oversized_batch_behavior()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut use_case = MockWithdrawUseCase::new();
        use_case
            .expect_execute_by_key()
            .times(100)
            .returning(|_, _| Ok(withdrawn()));
        let app = app(use_case);
        let source = ListingSourceId::new();
        let path = format!("/v1/listing-sources/{source}/product-listings");
        assert_eq!(
            StatusCode::UNAUTHORIZED,
            request(&app, &path, "[]", false).await?.status()
        );
        let empty = request(&app, &path, "[]", true).await?;
        assert_eq!(StatusCode::OK, empty.status());
        assert_eq!(json!([]), body_json(empty).await?);
        let item = r#"{"sourceListingId":"same"}"#.to_owned();
        let oversized = format!("[{}]", vec![item.clone(); 101].join(","));
        assert_eq!(
            StatusCode::BAD_REQUEST,
            request(&app, &path, &oversized, true).await?.status()
        );
        let maximum = format!("[{}]", vec![item; 100].join(","));
        let maximum = request(&app, &path, &maximum, true).await?;
        assert_eq!(StatusCode::OK, maximum.status());
        assert_eq!(json!([]), body_json(maximum).await?);
        Ok(())
    }

    #[tokio::test]
    async fn should_reject_later_conversion_failure_before_any_item_use_case()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut use_case = MockWithdrawUseCase::new();
        use_case.expect_execute_by_key().never();
        let app = app(use_case);
        let source = ListingSourceId::new();
        let response = request(
            &app,
            &format!("/v1/listing-sources/{source}/product-listings"),
            &format!(
                "[{},{}]",
                r#"{"sourceListingId":"same"}"#.to_owned(),
                r#"{"sourceListingId":"   "}"#
            ),
            true,
        )
        .await?;
        assert_eq!(StatusCode::BAD_REQUEST, response.status());
        assert_eq!("BAD_BODY_VALUE", body_json(response).await?["error"]);
        Ok(())
    }

    fn app(withdraw: MockWithdrawUseCase) -> Router {
        let state = PartnerProductListingsState::new(
            Arc::new(PartnerProductListingBatchesHandler::new(
                Arc::new(MockCreateUseCase::new()),
                Arc::new(MockUpdateUseCase::new()),
                Arc::new(MockUpsertUseCase::new()),
                Arc::new(withdraw),
            )),
            Arc::new(authenticator()),
        );
        Router::new()
            .route(
                "/v1/listing-sources/{listing_source_id}/product-listings",
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
            outcome: ChangeOutcome::Changed,
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

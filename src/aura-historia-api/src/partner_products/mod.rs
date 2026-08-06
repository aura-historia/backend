pub mod create_products;
pub mod delete_products;
pub mod update_products;
pub mod upsert_products;

mod types;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::{
        AuthError, AuthMethod, RequestMetadata, TokenAuthenticator, TransportPrincipal,
    };
    use crate::state::PartnerProductsState;
    use axum::Router;
    use axum::body::Body;
    use axum::http::{Request, StatusCode, header};
    use common::event_id::EventId;
    use common::operation_context::{CredentialCapability, OperationContext, Principal};
    use common::product_id::ProductId;
    use common::product_slug_id::ProductSlugId;
    use common::user_id::UserId;
    use product_service::use_cases::{
        CreateProductCommand, CreateProductError, CreateProductResult, CreateProductUseCase,
        DeleteProductCommand, DeleteProductError, DeleteProductResult, DeleteProductUseCase,
        UpdateProductCommand, UpdateProductError, UpdateProductResult, UpdateProductUseCase,
        UpsertProductCommand, UpsertProductError, UpsertProductResult, UpsertProductUseCase,
    };
    use serde_json::{Value, json};
    use std::collections::BTreeSet;
    use std::sync::{Arc, Mutex, MutexGuard};
    use tower::ServiceExt;

    #[derive(Default)]
    struct Calls {
        create: usize,
        update: usize,
        upsert: usize,
        delete: usize,
    }

    type SharedCalls = Arc<Mutex<Calls>>;

    #[derive(Clone)]
    struct FakeCreateUseCase {
        calls: SharedCalls,
        require_products_write: bool,
    }

    #[async_trait::async_trait]
    impl CreateProductUseCase for FakeCreateUseCase {
        async fn execute(
            &self,
            context: &OperationContext,
            command: CreateProductCommand,
        ) -> Result<CreateProductResult, CreateProductError> {
            lock(&self.calls).create += 1;
            if self.require_products_write && !has_products_write(&context.principal) {
                return Err(CreateProductError::Forbidden);
            }
            if command.shops_product_id.as_ref() == "failed" {
                return Err(CreateProductError::ShopProductAlreadyExists);
            }
            Ok(CreateProductResult {
                product_id: ProductId::new(),
                product_slug_id: ProductSlugId::from("created-product"),
                event_id: EventId::new(),
            })
        }
    }

    #[derive(Clone)]
    struct FakeUpdateUseCase {
        calls: SharedCalls,
    }

    #[async_trait::async_trait]
    impl UpdateProductUseCase for FakeUpdateUseCase {
        async fn execute(
            &self,
            _context: &OperationContext,
            _command: UpdateProductCommand,
        ) -> Result<UpdateProductResult, UpdateProductError> {
            lock(&self.calls).update += 1;
            Err(UpdateProductError::ProductNotFound)
        }
    }

    #[derive(Clone)]
    struct FakeUpsertUseCase {
        calls: SharedCalls,
    }

    #[async_trait::async_trait]
    impl UpsertProductUseCase for FakeUpsertUseCase {
        async fn execute(
            &self,
            _context: &OperationContext,
            _command: UpsertProductCommand,
        ) -> Result<UpsertProductResult, UpsertProductError> {
            lock(&self.calls).upsert += 1;
            Ok(UpsertProductResult::Created(CreateProductResult {
                product_id: ProductId::new(),
                product_slug_id: ProductSlugId::from("upserted-product"),
                event_id: EventId::new(),
            }))
        }
    }

    #[derive(Clone)]
    struct FakeDeleteUseCase {
        calls: SharedCalls,
    }

    #[async_trait::async_trait]
    impl DeleteProductUseCase for FakeDeleteUseCase {
        async fn execute(
            &self,
            _context: &OperationContext,
            _command: DeleteProductCommand,
        ) -> Result<DeleteProductResult, DeleteProductError> {
            lock(&self.calls).delete += 1;
            Ok(DeleteProductResult {
                product_id: ProductId::new(),
                event_id: EventId::new(),
            })
        }
    }

    struct FakeAuthenticator {
        capabilities: BTreeSet<CredentialCapability>,
    }

    #[async_trait::async_trait]
    impl TokenAuthenticator for FakeAuthenticator {
        async fn authenticate(
            &self,
            _bearer_token: &str,
            _metadata: &RequestMetadata,
        ) -> Result<TransportPrincipal, AuthError> {
            Ok(TransportPrincipal::User {
                user_id: UserId::new(),
                auth_method: AuthMethod::AuraAccessToken,
                capabilities: self.capabilities.clone(),
            })
        }
    }

    #[tokio::test]
    async fn should_return_empty_failures_when_all_creates_succeed()
    -> Result<(), Box<dyn std::error::Error>> {
        let shop_id = common::shop_id::ShopId::new();
        let (app, calls) = app(BTreeSet::from([CredentialCapability::ProductsWrite]), false);

        let response = request(
            &app,
            "POST",
            &format!("/api/v1/shops/{shop_id}/products"),
            create_body("created"),
            true,
        )
        .await?;

        assert_eq!(StatusCode::OK, response.status());
        assert_eq!(json!([]), body_json(response).await?);
        assert_eq!(1, lock(&calls).create);
        Ok(())
    }

    #[tokio::test]
    async fn should_return_failed_product_keys_when_create_batch_partially_succeeds()
    -> Result<(), Box<dyn std::error::Error>> {
        let shop_id = common::shop_id::ShopId::new();
        let (app, calls) = app(BTreeSet::from([CredentialCapability::ProductsWrite]), false);
        let body = format!("[{},{}]", create_object("created"), create_object("failed"));

        let response = request(
            &app,
            "POST",
            &format!("/api/v1/shops/{shop_id}/products"),
            body,
            true,
        )
        .await?;

        assert_eq!(StatusCode::OK, response.status());
        assert_eq!(
            json!([{"shopId": shop_id.to_string(), "shopsProductId": "failed"}]),
            body_json(response).await?
        );
        assert_eq!(2, lock(&calls).create);
        Ok(())
    }

    #[tokio::test]
    async fn should_return_first_error_when_all_updates_fail()
    -> Result<(), Box<dyn std::error::Error>> {
        let shop_id = common::shop_id::ShopId::new();
        let (app, calls) = app(BTreeSet::from([CredentialCapability::ProductsWrite]), false);

        let response = request(
            &app,
            "PATCH",
            &format!("/api/v1/shops/{shop_id}/products"),
            r#"[{"shopsProductId":"missing"}]"#.to_owned(),
            true,
        )
        .await?;

        assert_eq!(StatusCode::NOT_FOUND, response.status());
        assert_eq!("PRODUCT_NOT_FOUND", body_json(response).await?["error"]);
        assert_eq!(1, lock(&calls).update);
        Ok(())
    }

    #[tokio::test]
    async fn should_upsert_batch_on_collection_route() -> Result<(), Box<dyn std::error::Error>> {
        let shop_id = common::shop_id::ShopId::new();
        let (app, calls) = app(BTreeSet::from([CredentialCapability::ProductsWrite]), false);

        let response = request(
            &app,
            "PUT",
            &format!("/api/v1/shops/{shop_id}/products"),
            r#"[{"shopsProductId":"upserted"}]"#.to_owned(),
            true,
        )
        .await?;

        assert_eq!(StatusCode::OK, response.status());
        assert_eq!(json!([]), body_json(response).await?);
        assert_eq!(1, lock(&calls).upsert);
        Ok(())
    }

    #[tokio::test]
    async fn should_delete_batch_on_collection_route_and_not_expose_item_delete_route()
    -> Result<(), Box<dyn std::error::Error>> {
        let shop_id = common::shop_id::ShopId::new();
        let (app, calls) = app(BTreeSet::from([CredentialCapability::ProductsWrite]), false);

        let response = request(
            &app,
            "DELETE",
            &format!("/api/v1/shops/{shop_id}/products"),
            r#"[{"shopsProductId":"first"},{"shopsProductId":"second"}]"#.to_owned(),
            true,
        )
        .await?;

        assert_eq!(StatusCode::OK, response.status());
        assert_eq!(json!([]), body_json(response).await?);
        assert_eq!(2, lock(&calls).delete);

        let response = request(
            &app,
            "DELETE",
            &format!("/api/v1/shops/{shop_id}/products/first"),
            String::new(),
            true,
        )
        .await?;
        assert_eq!(StatusCode::NOT_FOUND, response.status());
        Ok(())
    }

    #[tokio::test]
    async fn should_reject_missing_bearer_token_before_invoking_create_use_case()
    -> Result<(), Box<dyn std::error::Error>> {
        let shop_id = common::shop_id::ShopId::new();
        let (app, calls) = app(BTreeSet::from([CredentialCapability::ProductsWrite]), false);

        let response = request(
            &app,
            "POST",
            &format!("/api/v1/shops/{shop_id}/products"),
            create_body("created"),
            false,
        )
        .await?;

        assert_eq!(StatusCode::UNAUTHORIZED, response.status());
        assert_eq!(0, lock(&calls).create);
        Ok(())
    }

    #[tokio::test]
    async fn should_reject_invalid_batch_json_before_invoking_create_use_case()
    -> Result<(), Box<dyn std::error::Error>> {
        let shop_id = common::shop_id::ShopId::new();
        let (app, calls) = app(BTreeSet::from([CredentialCapability::ProductsWrite]), false);

        let response = request(
            &app,
            "POST",
            &format!("/api/v1/shops/{shop_id}/products"),
            "{".to_owned(),
            true,
        )
        .await?;

        assert_eq!(StatusCode::BAD_REQUEST, response.status());
        assert_eq!(0, lock(&calls).create);
        Ok(())
    }

    #[tokio::test]
    async fn should_map_delegated_user_without_products_write_to_forbidden()
    -> Result<(), Box<dyn std::error::Error>> {
        let shop_id = common::shop_id::ShopId::new();
        let (app, calls) = app(BTreeSet::new(), true);

        let response = request(
            &app,
            "POST",
            &format!("/api/v1/shops/{shop_id}/products"),
            create_body("created"),
            true,
        )
        .await?;

        assert_eq!(StatusCode::FORBIDDEN, response.status());
        assert_eq!("FORBIDDEN", body_json(response).await?["error"]);
        assert_eq!(1, lock(&calls).create);
        Ok(())
    }

    fn app(
        capabilities: BTreeSet<CredentialCapability>,
        require_products_write: bool,
    ) -> (Router, SharedCalls) {
        let calls = Arc::new(Mutex::new(Calls::default()));
        let state = PartnerProductsState::new(
            Arc::new(FakeCreateUseCase {
                calls: Arc::clone(&calls),
                require_products_write,
            }),
            Arc::new(FakeUpdateUseCase {
                calls: Arc::clone(&calls),
            }),
            Arc::new(FakeUpsertUseCase {
                calls: Arc::clone(&calls),
            }),
            Arc::new(FakeDeleteUseCase {
                calls: Arc::clone(&calls),
            }),
            Arc::new(FakeAuthenticator { capabilities }),
        );
        (
            Router::new()
                .route(
                    "/api/v1/shops/{shop_id}/products",
                    axum::routing::post(create_products::create_products)
                        .patch(update_products::update_products)
                        .put(upsert_products::upsert_products)
                        .delete(delete_products::delete_products),
                )
                .with_state(state),
            calls,
        )
    }

    async fn request(
        app: &Router,
        method: &str,
        uri: &str,
        body: String,
        authenticated: bool,
    ) -> Result<axum::response::Response, Box<dyn std::error::Error>> {
        let mut builder = Request::builder().method(method).uri(uri);
        if authenticated {
            builder = builder.header(header::AUTHORIZATION, "Bearer valid");
        }
        Ok(app.clone().oneshot(builder.body(Body::from(body))?).await?)
    }

    async fn body_json(
        response: axum::response::Response,
    ) -> Result<Value, Box<dyn std::error::Error>> {
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX).await?;
        Ok(serde_json::from_slice(&bytes)?)
    }

    fn create_body(shops_product_id: &str) -> String {
        format!("[{}]", create_object(shops_product_id))
    }

    fn create_object(shops_product_id: &str) -> String {
        format!(
            r#"{{"shopsProductId":"{shops_product_id}","title":{{"text":"Cabinet","language":"en"}},"description":{{"text":"Old cabinet","language":"en"}},"state":"LISTED","url":"https://shop.example/products/{shops_product_id}","images":[]}}"#
        )
    }

    fn has_products_write(principal: &Principal) -> bool {
        match principal {
            Principal::DelegatedUser { capabilities, .. } => {
                capabilities.contains(&CredentialCapability::ProductsWrite)
            }
            Principal::User(_) | Principal::Service(_) | Principal::System => true,
            Principal::Anonymous => false,
        }
    }

    fn lock(calls: &SharedCalls) -> MutexGuard<'_, Calls> {
        match calls.lock() {
            Ok(calls) => calls,
            Err(poisoned) => poisoned.into_inner(),
        }
    }
}

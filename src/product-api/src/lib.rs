use aws_lambda_events::apigw::{ApiGatewayV2httpRequest, ApiGatewayV2httpResponse};
use cognito::access_token_verifier_service::AccessTokenVerifierService;
use common::api::{
    error::{ApiError, log_api_error},
    error_code::INTERNAL_SERVER_ERROR,
};
use embedding::EmbeddingGenerator;
use lambda_runtime::LambdaEvent;
use product::service::{
    get_service::GetProductService, query_service::QueryProductService,
    semantic_service::SemanticSearchService,
};
use product_personalization::service::ProductPersonalizationService;

pub mod get_product;
pub mod get_product_history;
pub mod get_product_similar;
pub mod search;

#[allow(clippy::too_many_arguments)]
#[tracing::instrument(
    skip(
        event,
        get_product_service,
        query_product_service,
        query_embedding_service,
        semantic_search_service,
        product_personalization_service,
        access_token_verifier_service,
    ),
    fields(
        requestId = %event.context.request_id,
        method = event.payload.request_context.http.method.as_str(),
        path = &event.payload.raw_path.as_deref().unwrap_or("NULL"),
        query = &event.payload.raw_query_string.as_deref().unwrap_or("NULL"),
        body = &event.payload.body.as_deref().unwrap_or("NULL"),
        ip = &event.payload.request_context.http.source_ip.as_deref().unwrap_or("NULL"),
        userAgent = &event.payload.request_context.http.user_agent.as_deref().unwrap_or("NULL"),
        userId = tracing::field::Empty,
    )
)]
pub async fn handler(
    event: LambdaEvent<ApiGatewayV2httpRequest>,
    get_product_service: &impl GetProductService,
    query_product_service: &impl QueryProductService,
    query_embedding_service: Option<&dyn EmbeddingGenerator>,
    semantic_search_service: &impl SemanticSearchService,
    product_personalization_service: &impl ProductPersonalizationService,
    access_token_verifier_service: &(impl AccessTokenVerifierService + Sync),
) -> Result<ApiGatewayV2httpResponse, lambda_runtime::Error> {
    match handle(
        event,
        get_product_service,
        query_product_service,
        query_embedding_service,
        semantic_search_service,
        product_personalization_service,
        access_token_verifier_service,
    )
    .await
    {
        Ok(response) => Ok(response),
        Err(err) => {
            log_api_error(&err);
            Ok(ApiGatewayV2httpResponse::from(err))
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub async fn handle(
    event: LambdaEvent<ApiGatewayV2httpRequest>,
    get_product_service: &impl GetProductService,
    query_product_service: &impl QueryProductService,
    query_embedding_service: Option<&dyn EmbeddingGenerator>,
    semantic_search_service: &impl SemanticSearchService,
    product_personalization_service: &impl ProductPersonalizationService,
    access_token_verifier_service: &(impl AccessTokenVerifierService + Sync),
) -> Result<ApiGatewayV2httpResponse, ApiError> {
    match event.payload.route_key.as_deref() {
        Some("GET /api/v1/shops/{shopId}/products/{shopsProductId}")
        | Some("GET /api/v1/by-slug/shops/{shopSlugId}/products/{productSlugId}") => {
            get_product::handle(
                event,
                get_product_service,
                access_token_verifier_service,
                product_personalization_service,
            )
            .await
        }
        Some("GET /api/v1/shops/{shopId}/products/{shopsProductId}/similar") => {
            get_product_similar::handle(
                event,
                semantic_search_service,
                access_token_verifier_service,
                product_personalization_service,
            )
            .await
        }
        Some("POST /api/v1/products/search") | Some("GET /api/v1/products") => {
            search::handle(
                event,
                query_product_service,
                query_embedding_service,
                access_token_verifier_service,
                product_personalization_service,
            )
            .await
        }
        Some("GET /api/v1/shops/{shopId}/products/{shopsProductId}/history") => {
            get_product_history::handle(event, get_product_service).await
        }
        Some(unknown) => Err(ApiError::internal_server_error(
            INTERNAL_SERVER_ERROR,
            format!("Unknown route-key '{unknown}' in AWS-Payload").into(),
        )),
        None => Err(ApiError::internal_server_error(
            INTERNAL_SERVER_ERROR,
            "Missing route-key in AWS-Payload".into(),
        )),
    }
}

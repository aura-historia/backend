use aws_lambda_events::apigw::{ApiGatewayV2httpRequest, ApiGatewayV2httpResponse};
use cognito::access_token_verifier_service::AccessTokenVerifierService;
use common::{
    api::{
        api_gateway_v2_http_response_builder::ApiGatewayV2HttpResponseBuilder,
        error::{ApiError, log_api_error},
    },
    currency::data::api::extract_currency_query,
    language::{data::api::extract_languages_header, domain::Language},
    personalized::{Personalized, api::PersonalizedData},
    shop_id::api::extract_shop_id_path,
    shops_item_id::api::extract_shops_item_id_path,
};
use item::core::user_state::ItemUserState;
use item::{
    data::get_data::GetItemData, service::personalization_service::ItemPersonalizationService,
};
use item::{
    data::user_state_data::ItemUserStateData, service::semantic_service::SemanticSearchService,
};
use lambda_runtime::LambdaEvent;

#[tracing::instrument(
    skip(event, semantic_search_service, access_token_verifier_service, item_personalization_service),
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
    semantic_search_service: &impl SemanticSearchService,
    access_token_verifier_service: &(impl AccessTokenVerifierService + Sync),
    item_personalization_service: &impl ItemPersonalizationService,
) -> Result<ApiGatewayV2httpResponse, lambda_runtime::Error> {
    match handle(
        event,
        semantic_search_service,
        access_token_verifier_service,
        item_personalization_service,
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

// GET /api/v1/items/{shopId}/{shopsItemId}/similar
pub async fn handle(
    event: LambdaEvent<ApiGatewayV2httpRequest>,
    semantic_search_service: &impl SemanticSearchService,
    access_token_verifier_service: &(impl AccessTokenVerifierService + Sync),
    item_personalization_service: &impl ItemPersonalizationService,
) -> Result<ApiGatewayV2httpResponse, ApiError> {
    let user_id_opt = access_token_verifier_service
        .verify_extract_user_id(&event.payload.headers)
        .await?;
    if let Some(user_id) = user_id_opt {
        tracing::Span::current().record("userId", user_id.to_string());
    }

    let languages = extract_languages_header(&event.payload.headers)?
        .into_iter()
        .map(Language::from)
        .collect::<Vec<_>>();
    let currency = extract_currency_query(&event.payload.query_string_parameters)?;
    let shop_id = extract_shop_id_path(&event.payload.path_parameters)?;
    let shops_item_id = extract_shops_item_id_path(&event.payload.path_parameters)?;

    let localized_similar_items = semantic_search_service
        .similar_items(&shop_id, &shops_item_id, &languages, &currency.into())
        .await?;
    let personalized_similar_items = match user_id_opt {
        None => localized_similar_items
            .into_iter()
            .map(|item| Personalized {
                item,
                user_state: None,
            })
            .collect(),
        Some(user_id) => item_personalization_service
            .personalize_all_watchlist(&user_id, localized_similar_items)
            .await?
            .into_iter()
            .map(|personalized_item| Personalized {
                item: personalized_item.item,
                user_state: personalized_item
                    .user_state
                    .map(|watchlist| ItemUserState { watchlist }),
            })
            .collect::<Vec<_>>(),
    };

    let similar_items_data: Vec<PersonalizedData<GetItemData, ItemUserStateData>> =
        personalized_similar_items
            .into_iter()
            .map(PersonalizedData::from)
            .collect();

    Ok(ApiGatewayV2HttpResponseBuilder::json(200)
        .body_serde(similar_items_data)?
        .build())
}

#[cfg(test)]
#[allow(clippy::too_many_arguments)]
mod tests {}

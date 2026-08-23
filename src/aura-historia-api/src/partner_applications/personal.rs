use super::types::{OwnPartnerApplicationData, PostApplicationData, PostPayloadData};
use super::util::{no_store, parse_id, parse_json};
use crate::auth::protected_context;
use crate::error::ApiError;
use crate::state::PartnerApplicationsState;
use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use shop_partner_service::use_cases::{
    CreatePartnerShopApplicationCommand, CreatePartnerShopApplicationPayload,
    GetPartnerShopApplicationRequest, ListPartnerShopApplicationsRequest, NewPartnerShopCommand,
    WithdrawPartnerShopApplicationCommand,
};

pub async fn list_me(
    State(state): State<PartnerApplicationsState>,
    headers: HeaderMap,
) -> Response {
    let (ctx, user_id) = match protected_context(state.authenticator.as_ref(), &headers).await {
        Ok(v) => v,
        Err(r) => return *r,
    };
    match state
        .list
        .execute(&ctx, ListPartnerShopApplicationsRequest { user_id })
        .await
    {
        Ok(r) => no_store(
            Json(
                r.items
                    .into_iter()
                    .map(OwnPartnerApplicationData::from)
                    .collect::<Vec<_>>(),
            )
            .into_response(),
        ),
        Err(e) => ApiError::from(e).into_response(),
    }
}

pub async fn get_me(
    State(state): State<PartnerApplicationsState>,
    headers: HeaderMap,
    Path(raw_id): Path<String>,
) -> Response {
    let (ctx, user_id) = match protected_context(state.authenticator.as_ref(), &headers).await {
        Ok(v) => v,
        Err(r) => return *r,
    };
    let application_id = match parse_id(&raw_id) {
        Ok(v) => v,
        Err(r) => return r,
    };
    match state
        .get
        .execute(
            &ctx,
            GetPartnerShopApplicationRequest {
                user_id,
                application_id,
            },
        )
        .await
    {
        Ok(r) => no_store(Json(OwnPartnerApplicationData::from(r.application)).into_response()),
        Err(e) => ApiError::from(e).into_response(),
    }
}

pub async fn post_me(
    State(state): State<PartnerApplicationsState>,
    headers: HeaderMap,
    body: String,
) -> Response {
    let (ctx, user_id) = match protected_context(state.authenticator.as_ref(), &headers).await {
        Ok(v) => v,
        Err(r) => return *r,
    };
    let data: PostApplicationData = match parse_json(&body) {
        Ok(v) => v,
        Err(r) => return r,
    };
    let payload = match data.payload {
        PostPayloadData::Existing { shop_id } => {
            CreatePartnerShopApplicationPayload::Existing { shop_id }
        }
        PostPayloadData::New {
            shop_name,
            shop_type,
            shop_domains,
            shopify_domain,
            shopify_currency,
            shopify_language,
            woocommerce_webhook_secret,
            woocommerce_currency,
            woocommerce_language,
            shop_url,
            shop_image,
            shop_structured_address,
            shop_phone,
            shop_email,
        } => CreatePartnerShopApplicationPayload::New(NewPartnerShopCommand {
            name: shop_name,
            shop_type: shop_type.into(),
            domains: shop_domains,
            shopify_domain,
            shopify_currency: shopify_currency.map(Into::into),
            shopify_language: shopify_language.map(Into::into),
            woocommerce_webhook_secret,
            woocommerce_currency: woocommerce_currency.map(Into::into),
            woocommerce_language: woocommerce_language.map(Into::into),
            url: shop_url,
            image: shop_image,
            structured_address: shop_structured_address.map(Into::into),
            phone: shop_phone,
            email: shop_email,
        }),
    };
    match state
        .create
        .execute(
            &ctx,
            CreatePartnerShopApplicationCommand {
                applicant_user_id: user_id,
                payload,
            },
        )
        .await
    {
        Ok(r) => (
            StatusCode::CREATED,
            Json(OwnPartnerApplicationData::from(r.application)),
        )
            .into_response(),
        Err(e) => ApiError::from(e).into_response(),
    }
}

pub async fn delete_me(
    State(state): State<PartnerApplicationsState>,
    headers: HeaderMap,
    Path(raw_id): Path<String>,
) -> Response {
    let (ctx, user_id) = match protected_context(state.authenticator.as_ref(), &headers).await {
        Ok(v) => v,
        Err(r) => return *r,
    };
    let application_id = match parse_id(&raw_id) {
        Ok(v) => v,
        Err(r) => return r,
    };
    match state
        .delete
        .execute(
            &ctx,
            WithdrawPartnerShopApplicationCommand {
                user_id,
                application_id,
            },
        )
        .await
    {
        Ok(_) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => ApiError::from(e).into_response(),
    }
}

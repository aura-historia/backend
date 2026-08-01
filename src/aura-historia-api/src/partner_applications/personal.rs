use super::types::{
    PartnerApplicationData, PatchApplicationData, PostApplicationData, PostPayloadData,
};
use super::util::{no_store, parse_id, parse_json};
use crate::auth::protected_context;
use crate::error::{ApiError, BAD_BODY_VALUE};
use crate::state::PartnerApplicationsState;
use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use shop_partner_service::use_cases::{
    CreatePartnerShopApplicationCommand, CreatePartnerShopApplicationPayload,
    DeletePartnerShopApplicationCommand, GetPartnerShopApplicationRequest,
    ListPartnerShopApplicationsRequest, MarkPartnerShopApplicationInReviewCommand,
};

pub async fn list_me(
    State(state): State<PartnerApplicationsState>,
    headers: HeaderMap,
) -> Response {
    let (ctx, user_id) = match protected_context(state.authenticator.as_ref(), &headers).await {
        Ok(v) => v,
        Err(r) => return r,
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
                    .map(PartnerApplicationData::from)
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
        Err(r) => return r,
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
        Ok(r) => no_store(Json(PartnerApplicationData::from(r.application)).into_response()),
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
        Err(r) => return r,
    };
    let data: PostApplicationData = match parse_json(&body) {
        Ok(v) => v,
        Err(r) => return r,
    };
    let payload = match data.payload {
        PostPayloadData::Existing { shop_id } | PostPayloadData::New { shop_id } => {
            CreatePartnerShopApplicationPayload::Existing { shop_id }
        }
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
            Json(PartnerApplicationData::from(r.application)),
        )
            .into_response(),
        Err(e) => ApiError::from(e).into_response(),
    }
}
pub async fn patch_me(
    State(state): State<PartnerApplicationsState>,
    headers: HeaderMap,
    Path(raw_id): Path<String>,
    body: String,
) -> Response {
    let (ctx, user_id) = match protected_context(state.authenticator.as_ref(), &headers).await {
        Ok(v) => v,
        Err(r) => return r,
    };
    let application_id = match parse_id(&raw_id) {
        Ok(v) => v,
        Err(r) => return r,
    };
    let data: PatchApplicationData = match parse_json(&body) {
        Ok(v) => v,
        Err(r) => return r,
    };
    let Some(task_token) = data.task_token else {
        return ApiError::bad_request(BAD_BODY_VALUE)
            .with_detail("Field 'taskToken' is required.")
            .into_response();
    };
    match state
        .update
        .mark_in_review(
            &ctx,
            MarkPartnerShopApplicationInReviewCommand {
                user_id,
                application_id,
                task_token,
            },
        )
        .await
    {
        Ok(r) => Json(PartnerApplicationData::from(r.application)).into_response(),
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
        Err(r) => return r,
    };
    let application_id = match parse_id(&raw_id) {
        Ok(v) => v,
        Err(r) => return r,
    };
    match state
        .delete
        .execute(
            &ctx,
            DeletePartnerShopApplicationCommand {
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

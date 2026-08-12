use super::types::{AdminPartnerApplicationData, DecisionData, PartnerApplicationDecisionData};
use super::util::{no_store, parse_id, parse_json};
use crate::auth::protected_context;
use crate::error::ApiError;
use crate::state::PartnerApplicationsState;
use axum::Json;
use axum::extract::{Path, State};
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Response};

use shop_partner_service::use_cases::{
    AdminDecidePartnerShopApplicationCommand, AdminGetPartnerShopApplicationRequest,
    AdminListPartnerShopApplicationsRequest, AdminMarkPartnerShopApplicationInReviewCommand,
    PartnerShopApplicationDecision,
};

pub async fn admin_list(
    State(state): State<PartnerApplicationsState>,
    headers: HeaderMap,
) -> Response {
    let (ctx, _) = match protected_context(state.authenticator.as_ref(), &headers).await {
        Ok(v) => v,
        Err(r) => return r,
    };
    match state
        .admin_list
        .execute(&ctx, AdminListPartnerShopApplicationsRequest::default())
        .await
    {
        Ok(r) => no_store(
            Json(
                r.items
                    .into_iter()
                    .map(AdminPartnerApplicationData::from)
                    .collect::<Vec<_>>(),
            )
            .into_response(),
        ),
        Err(e) => ApiError::from(e).into_response(),
    }
}
pub async fn admin_get(
    State(state): State<PartnerApplicationsState>,
    headers: HeaderMap,
    Path(raw_id): Path<String>,
) -> Response {
    let (ctx, _) = match protected_context(state.authenticator.as_ref(), &headers).await {
        Ok(v) => v,
        Err(r) => return r,
    };
    let application_id = match parse_id(&raw_id) {
        Ok(v) => v,
        Err(r) => return r,
    };
    match state
        .admin_get
        .execute(
            &ctx,
            AdminGetPartnerShopApplicationRequest { application_id },
        )
        .await
    {
        Ok(r) => no_store(Json(AdminPartnerApplicationData::from(r.application)).into_response()),
        Err(e) => ApiError::from(e).into_response(),
    }
}
pub async fn admin_patch(
    State(state): State<PartnerApplicationsState>,
    headers: HeaderMap,
    Path(raw_id): Path<String>,
) -> Response {
    let (ctx, _) = match protected_context(state.authenticator.as_ref(), &headers).await {
        Ok(v) => v,
        Err(r) => return r,
    };
    let application_id = match parse_id(&raw_id) {
        Ok(v) => v,
        Err(r) => return r,
    };
    match state
        .admin_update
        .mark_in_review(
            &ctx,
            AdminMarkPartnerShopApplicationInReviewCommand { application_id },
        )
        .await
    {
        Ok(r) => Json(AdminPartnerApplicationData::from(r.application)).into_response(),
        Err(e) => ApiError::from(e).into_response(),
    }
}
pub async fn admin_decision(
    State(state): State<PartnerApplicationsState>,
    headers: HeaderMap,
    Path(raw_id): Path<String>,
    body: String,
) -> Response {
    let (ctx, _) = match protected_context(state.authenticator.as_ref(), &headers).await {
        Ok(v) => v,
        Err(r) => return r,
    };
    let application_id = match parse_id(&raw_id) {
        Ok(v) => v,
        Err(r) => return r,
    };
    let data: DecisionData = match parse_json(&body) {
        Ok(v) => v,
        Err(r) => return r,
    };
    let decision = match data.decision {
        PartnerApplicationDecisionData::Approve => PartnerShopApplicationDecision::Approve,
        PartnerApplicationDecisionData::Reject => PartnerShopApplicationDecision::Reject,
    };
    match state
        .admin_decide
        .execute(
            &ctx,
            AdminDecidePartnerShopApplicationCommand {
                application_id,
                decision,
            },
        )
        .await
    {
        Ok(r) => Json(AdminPartnerApplicationData::from(r.application)).into_response(),
        Err(e) => ApiError::from(e).into_response(),
    }
}

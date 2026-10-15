use super::*;
use shipping_service::imports::{ChangeShippingConnectionCommand, ShippingConnectionView};
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ConnectionData {
    expected_revision: u64,
    integration: serde_json::Value,
    active: bool,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct IdentityQuery {
    connection_revision: u64,
    object_key: String,
}
fn connection_data(v: ShippingConnectionView) -> Result<serde_json::Value, ApiError> {
    Ok(
        serde_json::json!({"revision":v.revision,"integration":shipping_json::listing_delivery::encode_integration(&v.integration).map_err(|_|bad())?,"active":v.active,"connectedAt":v.connected_at.format(&time::format_description::well_known::Rfc3339).map_err(|_|bad())?}),
    )
}
pub async fn get_connection(
    State(s): State<ShippingState>,
    Path(source): Path<String>,
    headers: HeaderMap,
) -> Response {
    let (ctx, _) = match protected_context(s.authenticator.as_ref(), &headers).await {
        Ok(v) => v,
        Err(e) => return *e,
    };
    let source = match source_id(&source) {
        Ok(v) => v,
        Err(e) => return e.into_response(),
    };
    match s
        .imports
        .connection(&ctx, source)
        .await
        .map_err(ApiError::from)
        .and_then(|v| v.map(connection_data).transpose())
    {
        Ok(v) => Json(v).into_response(),
        Err(e) => e.into_response(),
    }
}
pub async fn change_connection(
    State(s): State<ShippingState>,
    Path(source): Path<String>,
    headers: HeaderMap,
    body: String,
) -> Response {
    let (ctx, _) = match protected_context(s.authenticator.as_ref(), &headers).await {
        Ok(v) => v,
        Err(e) => return *e,
    };
    let command = (|| {
        let d: ConnectionData = parse(&body)?;
        Ok::<_, ApiError>(ChangeShippingConnectionCommand {
            source_id: source_id(&source)?,
            expected_revision: d.expected_revision,
            integration: shipping_json::listing_delivery::decode_integration(d.integration)
                .map_err(|_| bad())?,
            active: d.active,
        })
    })();
    let command = match command {
        Ok(v) => v,
        Err(e) => return e.into_response(),
    };
    match s
        .imports
        .change_connection(&ctx, command)
        .await
        .map_err(ApiError::from)
        .and_then(connection_data)
    {
        Ok(v) => Json(v).into_response(),
        Err(e) => e.into_response(),
    }
}
pub async fn resolve_import_profile(
    State(s): State<ShippingState>,
    Path(source): Path<String>,
    headers: HeaderMap,
    Query(q): Query<IdentityQuery>,
) -> Response {
    let (ctx, _) = match protected_context(s.authenticator.as_ref(), &headers).await {
        Ok(v) => v,
        Err(e) => return *e,
    };
    let source = match source_id(&source) {
        Ok(v) => v,
        Err(e) => return e.into_response(),
    };
    match s
        .imports
        .resolve_profile(&ctx, source, q.connection_revision, q.object_key)
        .await
    {
        Ok(id) => Json(serde_json::json!({"shippingProfileId":id})).into_response(),
        Err(e) => ApiError::from(e).into_response(),
    }
}

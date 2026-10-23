use crate::{InvocationContext, Registry, http::message::RawToolArguments};
use rmcp::{
    ErrorData, RoleServer, ServerHandler,
    model::{
        CallToolRequestParams, CallToolResponse, Implementation, ListToolsResult,
        PaginatedRequestParams, ProtocolVersion, ServerCapabilities, ServerConfig, Tool,
    },
    service::RequestContext,
};
use std::borrow::Cow;

pub const SUPPORTED_PROTOCOL_VERSIONS: &[ProtocolVersion] = &[
    ProtocolVersion::V_2026_07_28,
    ProtocolVersion::V_2025_11_25,
    ProtocolVersion::V_2025_06_18,
    ProtocolVersion::V_2025_03_26,
];

pub(crate) fn valid_tool_parameters(method: &str, params: Option<&serde_json::Value>) -> bool {
    // Validate these SDK types directly: its untagged request parser treats
    // malformed calls as custom methods, and flattened optional list parameters
    // can silently discard an invalid cursor. Envelope/metadata parsing stays
    // with the SDK; HTTP admission must run before this parameter validation.
    match method {
        "tools/call" => params.is_some_and(|params| {
            serde_json::from_value::<CallToolRequestParams>(params.clone()).is_ok()
        }),
        "tools/list" => params.is_none_or(|params| {
            serde_json::from_value::<PaginatedRequestParams>(params.clone()).is_ok()
        }),
        _ => true,
    }
}

#[derive(Clone)]
pub(crate) struct ProtocolHandler {
    pub registry: Registry,
}

impl ServerHandler for ProtocolHandler {
    fn supported_protocol_versions(&self) -> Cow<'static, [ProtocolVersion]> {
        Cow::Borrowed(SUPPORTED_PROTOCOL_VERSIONS)
    }
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build()).with_server_info(
            Implementation::new("aura-historia-mcp", env!("CARGO_PKG_VERSION")),
        )
    }
    fn get_tool(&self, name: &str) -> Option<Tool> {
        self.registry.get(name).map(|op| op.tool.clone())
    }
    async fn list_tools(
        &self,
        params: Option<PaginatedRequestParams>,
        _: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        let (tools, next_cursor) = self
            .registry
            .page(params.as_ref().and_then(|p| p.cursor.as_deref()))?;
        let mut result = ListToolsResult::with_all_items(tools);
        result.next_cursor = next_cursor;
        Ok(result)
    }
    async fn call_tool(
        &self,
        params: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        let operation = self
            .registry
            .get(&params.name)
            .ok_or_else(|| ErrorData::invalid_params("Unknown tool", None))?;
        let metadata = context
            .extensions
            .get::<axum::http::request::Parts>()
            .and_then(|parts| parts.extensions.get::<InvocationContext>())
            .cloned()
            .ok_or_else(|| ErrorData::internal_error("Request context unavailable", None))?;
        let _cancel_on_drop = metadata.cancellation.clone().drop_guard();
        let raw = context
            .extensions
            .get::<axum::http::request::Parts>()
            .and_then(|parts| parts.extensions.get::<RawToolArguments>())
            .filter(|raw| raw.name == params.name)
            .ok_or_else(|| ErrorData::internal_error("Request arguments unavailable", None))?;
        // The raw decoder and SDK must dispatch the same immutable call identity.
        let projected: serde_json::Value = serde_json::from_str(raw.arguments.get())
            .map_err(|_| ErrorData::internal_error("Request arguments unavailable", None))?;
        if projected != serde_json::Value::Object(params.arguments.unwrap_or_default()) {
            return Err(ErrorData::internal_error(
                "Request arguments disagree",
                None,
            ));
        }
        let arguments = raw.arguments.clone();
        tokio::select! {
            _ = context.ct.cancelled() => Err(ErrorData::internal_error("Request cancelled", None)),
            _ = metadata.cancellation.cancelled() => Err(ErrorData::internal_error("Request cancelled", None)),
            result = tokio::time::timeout_at(metadata.deadline, operation.call(arguments, metadata.clone())) => {
                match result {
                    Ok(Ok(result)) => Ok(result.into()),
                    Ok(Err(error)) => error.into_protocol().map(Into::into),
                    Err(_) => Err(ErrorData::internal_error("Request deadline exceeded", None)),
                }
            }
        }
    }
}

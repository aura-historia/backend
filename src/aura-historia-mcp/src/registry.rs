use crate::{InvocationContext, OperationError, bounded_json};
use application::operation_context::CredentialCapability;
use rmcp::model::{CallToolResult, JsonObject, MetaObject, Tool, ToolAnnotations};
use schemars::{JsonSchema, generate::SchemaSettings};
use serde::{Serialize, de::DeserializeOwned};
use serde_json::{Value, json};
use std::{collections::BTreeMap, future::Future, pin::Pin, sync::Arc};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccessClass {
    Public,
    Protected,
}

/// Discovery and admission metadata. Application services remain the authority.
#[derive(Clone)]
pub struct OperationPolicy {
    pub access: AccessClass,
    pub required_scopes: Vec<CredentialCapability>,
    pub read_only: bool,
    pub destructive: bool,
    pub idempotent: bool,
    pub open_world: bool,
}

type OperationFuture = Pin<Box<dyn Future<Output = Result<CallToolResult, OperationError>> + Send>>;
type Handler = dyn Fn(Value, InvocationContext) -> OperationFuture + Send + Sync;

#[derive(Clone)]
pub struct OperationDefinition {
    pub(crate) tool: Tool,
    policy: OperationPolicy,
    handler: Arc<Handler>,
}

/// Object envelope required by legacy MCP output schemas for arrays, null and scalars.
/// The inner value is the shared API response type without remapping or field dropping.
#[derive(Serialize, JsonSchema)]
struct ToolOutput<T> {
    result: T,
}

impl OperationDefinition {
    /// `I` is an object made from shared request types; `O` is the shared response type.
    /// The closure captures only the service handles this operation needs.
    pub fn new<I, O, F, Fut>(
        operation_id: &'static str,
        description: &'static str,
        policy: OperationPolicy,
        handler: F,
    ) -> Result<Self, RegistryError>
    where
        I: DeserializeOwned + JsonSchema + Send + 'static,
        O: Serialize + JsonSchema + Send + 'static,
        F: Fn(I, InvocationContext) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<O, OperationError>> + Send + 'static,
    {
        if operation_id.is_empty()
            || operation_id.len() > 128
            || !operation_id
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'-' | b'.'))
        {
            return Err(RegistryError::Name);
        }
        if policy.read_only && policy.destructive {
            return Err(RegistryError::Annotations);
        }
        let input = schema::<I>(SchemaSettings::default().for_deserialize())?;
        if input.get("type").and_then(Value::as_str) != Some("object") {
            return Err(RegistryError::ObjectInput);
        }
        let output = schema::<O>(SchemaSettings::default().for_serialize())?;
        let direct_output = output.get("type").and_then(Value::as_str) == Some("object");
        let output = if direct_output {
            output
        } else {
            schema::<ToolOutput<O>>(SchemaSettings::default().for_serialize())?
        };
        let mut tool = Tool::new(operation_id, description, input);
        tool.output_schema = Some(Arc::new(output));
        tool.annotations = Some(
            ToolAnnotations::new()
                .read_only(policy.read_only)
                .destructive(policy.destructive)
                .idempotent(policy.idempotent)
                .open_world(policy.open_world),
        );
        tool.meta = Some(MetaObject(serde_json::from_value(json!({
            "aura.historia/operationId": operation_id,
            "aura.historia/access": match policy.access { AccessClass::Public => "public", AccessClass::Protected => "protected" },
            "aura.historia/requiredScopes": policy.required_scopes.iter().map(|s| s.as_str()).collect::<Vec<_>>()
        })).map_err(RegistryError::Schema)?));
        Ok(Self {
            tool,
            policy,
            handler: typed_handler(
                handler,
                if direct_output {
                    OutputEncoding::Direct
                } else {
                    OutputEncoding::Wrapped
                },
            ),
        })
    }

    pub fn name(&self) -> &str {
        &self.tool.name
    }
    pub fn policy(&self) -> &OperationPolicy {
        &self.policy
    }
    pub fn tool(&self) -> &Tool {
        &self.tool
    }
    pub(crate) async fn call(
        &self,
        input: Value,
        context: InvocationContext,
    ) -> Result<CallToolResult, OperationError> {
        (self.handler)(input, context).await
    }
}

#[derive(Clone, Default)]
pub struct Registry {
    operations: Arc<BTreeMap<String, OperationDefinition>>,
}

impl Registry {
    pub fn new(
        operations: impl IntoIterator<Item = OperationDefinition>,
    ) -> Result<Self, RegistryError> {
        let mut registered = BTreeMap::new();
        for operation in operations {
            if registered
                .insert(operation.name().to_owned(), operation)
                .is_some()
            {
                return Err(RegistryError::Duplicate);
            }
        }
        Ok(Self {
            operations: Arc::new(registered),
        })
    }
    pub fn get(&self, name: &str) -> Option<&OperationDefinition> {
        self.operations.get(name)
    }
    pub(crate) fn page(
        &self,
        cursor: Option<&str>,
    ) -> Result<(Vec<Tool>, Option<String>), rmcp::ErrorData> {
        if cursor.is_some_and(|cursor| !self.operations.contains_key(cursor)) {
            return Err(rmcp::ErrorData::invalid_params(
                "Invalid tools cursor",
                None,
            ));
        }
        let mut remaining = self
            .operations
            .iter()
            .filter(|(name, _)| cursor.is_none_or(|cursor| name.as_str() > cursor));
        let tools: Vec<_> = remaining
            .by_ref()
            .take(50)
            .map(|(_, op)| op.tool.clone())
            .collect();
        let next = remaining
            .next()
            .and_then(|_| tools.last().map(|tool| tool.name.to_string()));
        Ok((tools, next))
    }
}

fn schema<T: JsonSchema>(settings: SchemaSettings) -> Result<JsonObject, RegistryError> {
    serde_json::to_value(settings.into_generator().into_root_schema_for::<T>())
        .map_err(RegistryError::Schema)?
        .as_object()
        .cloned()
        .ok_or(RegistryError::ObjectInput)
}

#[derive(Debug, thiserror::Error)]
pub enum RegistryError {
    #[error("tool name must be the source operation ID using supported ASCII characters")]
    Name,
    #[error("duplicate tool operation ID")]
    Duplicate,
    #[error("read-only tool cannot be destructive")]
    Annotations,
    #[error("tool arguments must have an object schema")]
    ObjectInput,
    #[error("failed to generate tool schema")]
    Schema(#[source] serde_json::Error),
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum OutputEncoding {
    Direct,
    Wrapped,
}

fn typed_handler<I, O, F, Fut>(handler: F, encoding: OutputEncoding) -> Arc<Handler>
where
    I: DeserializeOwned + Send + 'static,
    O: Serialize + Send + 'static,
    F: Fn(I, InvocationContext) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = Result<O, OperationError>> + Send + 'static,
{
    let handler = Arc::new(handler);
    Arc::new(move |arguments, context| {
        let handler = Arc::clone(&handler);
        Box::pin(async move {
            let input = serde_json::from_value(arguments)
                .map_err(|error| OperationError::InvalidArguments(Box::new(error)))?;
            let limit = context.max_response_bytes;
            let value = handler(input, context).await?;
            let bytes = if encoding == OutputEncoding::Direct {
                bounded_json::serialize(&value, limit)
            } else {
                bounded_json::serialize(&ToolOutput { result: value }, limit)
            }
            .map_err(bounded_json::SerializationError::into_operation)?;
            let value = serde_json::from_slice(&bytes)
                .map_err(|error| OperationError::Internal(Box::new(error)))?;
            let result = CallToolResult::structured(value);
            // Includes both structured data and the legacy text representation.
            bounded_json::serialize(&result, limit.saturating_sub(512))
                .map_err(bounded_json::SerializationError::into_operation)?;
            Ok(result)
        })
    })
}

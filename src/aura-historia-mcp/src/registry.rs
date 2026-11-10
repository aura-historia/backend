use crate::{InvocationContext, OperationError, bounded_json};
use application::operation_context::CredentialCapability;
use aura_historia_contracts::wire::description::MINOR_UNIT_RULE;
use rmcp::model::{CallToolResult, JsonObject, MetaObject, Tool, ToolAnnotations};
use schemars::{JsonSchema, generate::SchemaSettings};
use serde::{Serialize, de::DeserializeOwned};
use serde_json::{Value, json, value::RawValue};
use std::{
    borrow::Cow,
    collections::{BTreeMap, BTreeSet},
    future::Future,
    pin::Pin,
    sync::Arc,
};

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
type Handler = dyn Fn(Box<RawValue>, InvocationContext) -> OperationFuture + Send + Sync;

#[derive(Clone)]
pub struct OperationDefinition {
    pub(crate) tool: Tool,
    policy: OperationPolicy,
    handler: Arc<Handler>,
}

/// Structured result of a bodyless business outcome: an empty object rather than `null`,
/// because tool results never carry JSON `null` members.
#[derive(Debug, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Completed {}

/// Object envelope required by legacy MCP output schemas for arrays and scalars.
/// The inner value is the shared API response type without remapping or field dropping.
#[derive(Serialize, JsonSchema)]
struct ToolOutput<T> {
    result: T,
}

impl OperationDefinition {
    /// `I` is an object made from shared request types; `O` is the shared response type.
    /// The closure captures only the service handles this operation needs. The name stays the
    /// source operation ID; `title` is the short display name clients show instead.
    pub fn new<I, O, F, Fut>(
        operation_id: &'static str,
        title: &'static str,
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
        if !valid_operation_id(operation_id) {
            return Err(RegistryError::Name);
        }
        if !valid_title(title) {
            return Err(RegistryError::Title);
        }
        if policy.read_only && policy.destructive {
            return Err(RegistryError::Annotations);
        }
        let mut input = schema::<I>(SchemaSettings::default().for_deserialize())?;
        if !normalize_object_schema(&mut input) {
            return Err(RegistryError::ObjectInput);
        }
        declare_referenced_property_types(&mut input);
        let mut output = schema::<O>(SchemaSettings::default().for_serialize())?;
        let direct_output = normalize_object_schema(&mut output);
        let output = if direct_output {
            output
        } else {
            schema::<ToolOutput<O>>(SchemaSettings::default().for_serialize())?
        };
        let description = with_amount_rule(description, &input, &output);
        let mut tool = Tool::new(operation_id, description, input).with_title(title);
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
    pub fn title(&self) -> &str {
        self.tool.title.as_deref().unwrap_or_default()
    }
    pub fn policy(&self) -> &OperationPolicy {
        &self.policy
    }
    pub fn tool(&self) -> &Tool {
        &self.tool
    }
    pub(crate) async fn call(
        &self,
        input: impl Serialize,
        context: InvocationContext,
    ) -> Result<CallToolResult, OperationError> {
        let raw = serde_json::value::to_raw_value(&input)
            .map_err(|error| OperationError::InvalidArguments(Box::new(error)))?;
        (self.handler)(raw, context).await
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
        let mut titles = BTreeSet::new();
        for operation in operations {
            let title = operation.title().to_lowercase();
            if registered
                .insert(operation.name().to_owned(), operation)
                .is_some()
            {
                return Err(RegistryError::Duplicate);
            }
            if !titles.insert(title) {
                return Err(RegistryError::DuplicateTitle);
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

fn valid_operation_id(operation_id: &str) -> bool {
    !operation_id.is_empty()
        && operation_id.len() <= 128
        && operation_id
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'-' | b'.'))
}

/// Directory reviews flag tools without a display title; a long one is a misplaced description.
fn valid_title(title: &str) -> bool {
    !title.is_empty()
        && title.chars().count() <= MAX_TITLE_CHARS
        && title.trim() == title
        && !title.chars().any(char::is_control)
}

const MAX_TITLE_CHARS: usize = 64;

fn schema<T: JsonSchema>(settings: SchemaSettings) -> Result<JsonObject, RegistryError> {
    serde_json::to_value(settings.into_generator().into_root_schema_for::<T>())
        .map_err(RegistryError::Schema)?
        .as_object()
        .cloned()
        .ok_or(RegistryError::ObjectInput)
}

fn normalize_object_schema(schema: &mut JsonObject) -> bool {
    if !has_only_object_variants(schema) {
        return false;
    }
    // Schemars describes tagged object enums with oneOf, omitting the root type.
    // Legacy MCP requires that explicit root even when every variant is an object.
    schema.insert("type".into(), json!("object"));
    true
}

fn has_only_object_variants(schema: &JsonObject) -> bool {
    schema.get("type").and_then(Value::as_str) == Some("object")
        || ["oneOf", "anyOf"].iter().any(|keyword| {
            schema
                .get(*keyword)
                .and_then(Value::as_array)
                .is_some_and(|variants| {
                    !variants.is_empty()
                        && variants.iter().all(|variant| {
                            variant.as_object().is_some_and(has_only_object_variants)
                        })
                })
        })
}

/// A tool whose arguments or result contain a money amount states how amounts are encoded, from
/// the one shared rule, so no price-handling tool can leave it out.
fn with_amount_rule(
    description: &'static str,
    input: &JsonObject,
    output: &JsonObject,
) -> Cow<'static, str> {
    if object_declares_money(input) || object_declares_money(output) {
        Cow::Owned(format!("{description} {MINOR_UNIT_RULE}"))
    } else {
        Cow::Borrowed(description)
    }
}

/// A money amount is an object with `amount` and `currency` members; a distance has `unit`
/// instead of `currency` and is not one.
pub(crate) fn declares_money(schema: &Value) -> bool {
    match schema {
        Value::Object(object) => object_declares_money(object),
        Value::Array(items) => items.iter().any(declares_money),
        _ => false,
    }
}

fn object_declares_money(object: &JsonObject) -> bool {
    object
        .get("properties")
        .and_then(Value::as_object)
        .is_some_and(|properties| {
            properties.contains_key("amount") && properties.contains_key("currency")
        })
        || object.values().any(declares_money)
}

/// Longest `$ref` / `allOf` / `oneOf` chain followed to find a definition's type. Generated
/// schemas nest only a few levels; the bound exists to end a reference cycle.
const MAX_TYPE_LOOKUP_DEPTH: usize = 8;

/// Schemars describes an argument whose type is a shared definition as a bare `$ref`, or as an
/// `allOf` that refines one, and leaves out its `type`. The Claude connector review flags such
/// arguments, so the type the definition already declares is repeated next to the reference. It
/// never narrows what the argument accepts. Only the tool's own arguments are completed, not the
/// properties inside shared definitions. An argument whose type cannot be derived stays as
/// generated, and the production registry test reports it.
fn declare_referenced_property_types(schema: &mut JsonObject) {
    let Some(Value::Object(properties)) = schema.get("properties") else {
        return;
    };
    let declarations: Vec<(String, Value)> = properties
        .iter()
        .filter_map(|(name, property)| {
            let property = property.as_object()?;
            let is_reference = property.contains_key("$ref") || property.contains_key("allOf");
            if !is_reference || property.contains_key("type") {
                return None;
            }
            let declared = declared_type(property, schema, 0)?;
            Some((name.clone(), declared))
        })
        .collect();
    if let Some(Value::Object(properties)) = schema.get_mut("properties") {
        for (name, declared) in declarations {
            if let Some(Value::Object(property)) = properties.get_mut(&name) {
                property.insert("type".into(), declared);
            }
        }
    }
}

/// The `type` a schema declares itself, through its `$ref` target or `allOf` members, or the one
/// that every `oneOf` / `anyOf` variant agrees on.
fn declared_type(schema: &JsonObject, root: &JsonObject, depth: usize) -> Option<Value> {
    if depth > MAX_TYPE_LOOKUP_DEPTH {
        return None;
    }
    if let Some(declared) = schema.get("type") {
        return Some(declared.clone());
    }
    if let Some(reference) = schema.get("$ref").and_then(Value::as_str) {
        let definition = reference.strip_prefix("#/$defs/")?;
        let target = root.get("$defs")?.get(definition)?.as_object()?;
        return declared_type(target, root, depth + 1);
    }
    if let Some(members) = schema.get("allOf").and_then(Value::as_array) {
        // Every member applies, so the type of any one of them already holds for the whole.
        return members
            .iter()
            .filter_map(Value::as_object)
            .find_map(|member| declared_type(member, root, depth + 1));
    }
    ["oneOf", "anyOf"].iter().find_map(|keyword| {
        let variants = schema.get(*keyword)?.as_array()?;
        let mut types = variants.iter().map(|variant| {
            variant
                .as_object()
                .and_then(|variant| declared_type(variant, root, depth + 1))
        });
        let shared = types.next()??;
        types
            .all(|other| other.as_ref() == Some(&shared))
            .then_some(shared)
    })
}

#[derive(Debug, thiserror::Error)]
pub enum RegistryError {
    #[error("tool name must be the source operation ID using supported ASCII characters")]
    Name,
    #[error("duplicate tool operation ID")]
    Duplicate,
    #[error("tool title must be a nonempty, trimmed display name of at most 64 characters")]
    Title,
    #[error("duplicate tool title")]
    DuplicateTitle,
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
            let input = crate::arguments::decode(arguments)
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;

    fn declare(mut schema: Value) -> Value {
        declare_referenced_property_types(schema.as_object_mut().unwrap());
        schema
    }

    #[test]
    fn should_declare_the_object_type_of_a_referenced_definition() {
        let schema = declare(json!({
            "type": "object",
            "properties": {"range": {"$ref": "#/$defs/Range", "default": null}},
            "$defs": {"Range": {"type": "object", "properties": {}}}
        }));

        assert_eq!(
            json!({"$ref": "#/$defs/Range", "default": null, "type": "object"}),
            schema["properties"]["range"]
        );
    }

    #[test]
    fn should_declare_the_scalar_type_of_a_referenced_definition() {
        let schema = declare(json!({
            "type": "object",
            "properties": {"feedback": {"$ref": "#/$defs/PatchValue_boolean"}},
            "$defs": {"PatchValue_boolean": {"type": ["boolean", "null"]}}
        }));

        assert_eq!(
            json!(["boolean", "null"]),
            schema["properties"]["feedback"]["type"]
        );
    }

    #[test]
    fn should_declare_the_type_every_variant_of_a_referenced_definition_shares() {
        for keyword in ["oneOf", "anyOf"] {
            let schema = declare(json!({
                "type": "object",
                "properties": {"destination": {"$ref": "#/$defs/Destination"}},
                "$defs": {"Destination": {(keyword): [
                    {"type": "object", "properties": {"kind": {"const": "NONE"}}},
                    {"type": "object", "properties": {"kind": {"const": "COUNTRY"}}}
                ]}}
            }));

            assert_eq!(
                json!("object"),
                schema["properties"]["destination"]["type"],
                "{keyword}"
            );
        }
    }

    #[test]
    fn should_declare_the_type_of_a_refined_definition() {
        let schema = declare(json!({
            "type": "object",
            "properties": {"geography": {"allOf": [
                {"$ref": "#/$defs/Geography"},
                {"properties": {"addressText": {"type": "string"}}, "required": ["addressText"]}
            ]}},
            "$defs": {"Geography": {"type": "object", "properties": {}}}
        }));

        assert_eq!(json!("object"), schema["properties"]["geography"]["type"]);
    }

    #[test]
    fn should_keep_arguments_whose_type_is_declared_or_not_derivable() {
        let original = json!({
            "type": "object",
            "properties": {
                "declared": {"$ref": "#/$defs/Range", "type": "object"},
                "nullable": {"anyOf": [{"$ref": "#/$defs/Range"}, {"type": "null"}]},
                "mixed": {"$ref": "#/$defs/Mixed"},
                "mostly": {"$ref": "#/$defs/Mostly"},
                "missing": {"$ref": "#/$defs/Missing"},
                "foreign": {"$ref": "https://example.test/schema.json"},
                "loop": {"$ref": "#/$defs/Loop"}
            },
            "$defs": {
                "Range": {"type": "object"},
                "Mixed": {"oneOf": [{"type": "string"}, {"type": "integer"}]},
                "Mostly": {"oneOf": [{"type": "object"}, {"type": "object"}, {"type": "string"}]},
                "Loop": {"$ref": "#/$defs/Loop"}
            }
        });

        assert_eq!(original, declare(original.clone()));
    }

    #[derive(Deserialize, JsonSchema)]
    struct Window {
        #[allow(dead_code)]
        min: Option<u64>,
    }

    #[derive(Deserialize, JsonSchema)]
    struct WindowInput {
        #[allow(dead_code)]
        window: Window,
    }

    #[test]
    fn should_publish_declared_argument_types_for_a_registered_operation() {
        let operation = OperationDefinition::new::<WindowInput, Completed, _, _>(
            "windowOperation",
            "Window operation",
            "Takes a window.",
            OperationPolicy {
                access: AccessClass::Public,
                required_scopes: Vec::new(),
                read_only: true,
                destructive: false,
                idempotent: true,
                open_world: false,
            },
            |_, _| async { Ok(Completed {}) },
        )
        .unwrap();

        let schema = serde_json::to_value(&operation.tool().input_schema).unwrap();

        assert_eq!(
            json!({"$ref": "#/$defs/Window", "type": "object"}),
            schema["properties"]["window"]
        );
    }
}

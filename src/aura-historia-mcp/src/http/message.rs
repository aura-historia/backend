//! Retain tool argument bytes while validating the identity used by HTTP admission and the SDK.
use super::response::protocol_error;
use axum::{body::Bytes, http::StatusCode, response::Response};
use serde::{
    Deserialize,
    de::{MapAccess, Visitor},
};
use serde_json::{Value, json, value::RawValue};
use std::{collections::BTreeMap, fmt, sync::Arc};

#[derive(Clone)]
pub(crate) struct RawToolArguments {
    pub name: String,
    pub arguments: Arc<RawValue>,
}

pub(super) struct DecodedMessage {
    pub bytes: Bytes,
    pub body: Value,
    pub method: String,
    pub id: Value,
    pub arguments: Option<RawToolArguments>,
}

// Preserve raw values and reject ambiguous envelope fields before either admission or SDK parsing.
struct UniqueObject(BTreeMap<String, Box<RawValue>>);
impl<'de> Deserialize<'de> for UniqueObject {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct ObjectVisitor;
        impl<'de> Visitor<'de> for ObjectVisitor {
            type Value = UniqueObject;
            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                formatter.write_str("an object with unique fields")
            }
            fn visit_map<M: MapAccess<'de>>(self, mut map: M) -> Result<Self::Value, M::Error> {
                let mut fields = BTreeMap::new();
                while let Some((key, value)) = map.next_entry::<String, Box<RawValue>>()? {
                    if fields.insert(key, value).is_some() {
                        return Err(serde::de::Error::custom("duplicate envelope field"));
                    }
                }
                Ok(UniqueObject(fields))
            }
        }
        deserializer.deserialize_map(ObjectVisitor)
    }
}

pub(super) fn decode_message(bytes: Bytes) -> Result<DecodedMessage, Box<Response>> {
    let body: Value = serde_json::from_slice(&bytes).map_err(|_| {
        Box::new(protocol_error(
            StatusCode::BAD_REQUEST,
            -32700,
            "Invalid JSON",
            Value::Null,
        ))
    })?;
    let envelope: UniqueObject = serde_json::from_slice(&bytes).map_err(|_| invalid_envelope())?;
    let Some(method) = body
        .get("method")
        .and_then(Value::as_str)
        .filter(|_| body.get("jsonrpc") == Some(&json!("2.0")))
    else {
        return Err(invalid_envelope());
    };
    let method = method.to_owned();
    let id = body.get("id").cloned().unwrap_or(Value::Null);
    if body.get("id").is_some()
        && serde_json::from_value::<rmcp::model::RequestId>(id.clone()).is_err()
    {
        return Err(Box::new(protocol_error(
            StatusCode::BAD_REQUEST,
            -32600,
            "Invalid JSON-RPC request ID",
            Value::Null,
        )));
    }
    let arguments = raw_tool_arguments(&method, &body, &envelope)?;
    Ok(DecodedMessage {
        bytes,
        body,
        method,
        id,
        arguments,
    })
}

fn raw_tool_arguments(
    method: &str,
    body: &Value,
    envelope: &UniqueObject,
) -> Result<Option<RawToolArguments>, Box<Response>> {
    if !body.get("params").is_some_and(Value::is_object) {
        return Ok(None);
    }
    let Some(params) = envelope.0.get("params") else {
        return Ok(None);
    };
    let params: UniqueObject =
        serde_json::from_str(params.get()).map_err(|_| invalid_envelope())?;
    if let Some(metadata) = params.0.get("_meta") {
        validate_metadata(metadata).map_err(|_| invalid_envelope())?;
    }
    if method != "tools/call" {
        return Ok(None);
    }
    let Some(name) = body.pointer("/params/name").and_then(Value::as_str) else {
        return Ok(None);
    };
    let arguments = params
        .0
        .get("arguments")
        .filter(|raw| raw.get() != "null")
        .map_or_else(
            || RawValue::from_string("{}".to_owned()).expect("static object"),
            |raw| raw.clone(),
        );
    Ok(Some(RawToolArguments {
        name: name.to_owned(),
        arguments: arguments.into(),
    }))
}

fn validate_metadata(raw: &RawValue) -> Result<(), serde_json::Error> {
    match raw.get().as_bytes().first() {
        Some(b'{') => {
            let object: UniqueObject = serde_json::from_str(raw.get())?;
            for value in object.0.values() {
                validate_metadata(value)?;
            }
        }
        Some(b'[') => {
            let array: Vec<Box<RawValue>> = serde_json::from_str(raw.get())?;
            for value in array {
                validate_metadata(&value)?;
            }
        }
        _ => {}
    }
    Ok(())
}

fn invalid_envelope() -> Box<Response> {
    Box::new(protocol_error(
        StatusCode::BAD_REQUEST,
        -32600,
        "Expected one unambiguous JSON-RPC request or notification",
        Value::Null,
    ))
}

use crate::{TransportConfig, config::normalized_origin};
use axum::http::{HeaderMap, StatusCode, header};
use base64::{Engine, engine::general_purpose::STANDARD};
use serde_json::Value;

pub(super) fn validate_routing_headers(
    headers: &HeaderMap,
    body: &Value,
    method: &str,
) -> Result<(), &'static str> {
    let version = optional_header(headers, "mcp-protocol-version")?;
    let body_version = if method == "initialize" {
        body.pointer("/params/protocolVersion")
    } else {
        body.pointer("/params/_meta/io.modelcontextprotocol~1protocolVersion")
    }
    .and_then(Value::as_str);
    if method != "initialize" && body_version.is_some() && version.is_none() {
        return Err("Missing protocol version header");
    }
    if matches!((version, body_version), (Some(header), Some(body)) if header != body) {
        return Err("Protocol version header does not match body");
    }
    let modern = version
        .or(body_version)
        .is_some_and(|version| version >= "2026-07-28")
        && method != "initialize";
    match optional_header(headers, "mcp-method")? {
        Some(value) if value != method => return Err("Method header does not match body"),
        None if modern && body.get("id").is_some() => return Err("Missing method header"),
        _ => {}
    }
    let body_name = match method {
        "tools/call" | "prompts/get" => body.pointer("/params/name"),
        "resources/read" | "resources/subscribe" | "resources/unsubscribe" => {
            body.pointer("/params/uri")
        }
        "tasks/get" | "tasks/update" | "tasks/cancel" => body.pointer("/params/taskId"),
        _ => None,
    }
    .and_then(Value::as_str);
    match optional_header(headers, "mcp-name")? {
        Some(value) if decode_name(value)?.as_deref() != body_name => {
            return Err("Name header does not match body");
        }
        None if modern && body_name.is_some() => return Err("Missing name header"),
        _ => {}
    }
    // Credentials and routing metadata must never have ambiguous multiple values.
    for name in ["authorization", "origin", "host", "mcp-session-id"] {
        optional_header(headers, name)?;
    }
    Ok(())
}

fn decode_name(value: &str) -> Result<Option<String>, &'static str> {
    let decoded = if let Some(encoded) = value
        .strip_prefix("=?base64?")
        .and_then(|s| s.strip_suffix("?="))
    {
        String::from_utf8(
            STANDARD
                .decode(encoded)
                .map_err(|_| "Malformed name header")?,
        )
        .map_err(|_| "Malformed name header")?
    } else {
        value.to_owned()
    };
    Ok(Some(decoded))
}

pub(super) fn optional_header<'a>(
    headers: &'a HeaderMap,
    name: &str,
) -> Result<Option<&'a str>, &'static str> {
    let mut values = headers.get_all(name).iter();
    let value = values.next();
    if values.next().is_some() {
        return Err("Duplicate routing header");
    }
    value
        .map(|value| value.to_str().map_err(|_| "Malformed routing header"))
        .transpose()
}

pub(super) fn validate_origin_and_host(
    headers: &HeaderMap,
    config: &TransportConfig,
) -> Result<(), StatusCode> {
    let host = optional_header(headers, "host")
        .map_err(|_| StatusCode::FORBIDDEN)?
        .ok_or(StatusCode::FORBIDDEN)?;
    if !config
        .allowed_hosts
        .iter()
        .any(|allowed| allowed.eq_ignore_ascii_case(host))
    {
        return Err(StatusCode::FORBIDDEN);
    }
    if let Some(origin) = optional_header(headers, "origin").map_err(|_| StatusCode::FORBIDDEN)? {
        let origin = normalized_origin(origin).ok_or(StatusCode::FORBIDDEN)?;
        if !config
            .allowed_origins
            .iter()
            .any(|allowed| normalized_origin(allowed).as_ref() == Some(&origin))
        {
            return Err(StatusCode::FORBIDDEN);
        }
    }
    Ok(())
}

pub(super) fn valid_identifier(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'-' | b'_' | b'.'))
}

pub(super) fn valid_media_type(
    headers: &HeaderMap,
    name: header::HeaderName,
    expected: &str,
) -> bool {
    let mut values = headers.get_all(name).iter();
    let Some(value) = values.next().and_then(|v| v.to_str().ok()) else {
        return false;
    };
    values.next().is_none()
        && value
            .split(';')
            .next()
            .is_some_and(|v| v.trim().eq_ignore_ascii_case(expected))
}

pub(super) fn valid_accept(headers: &HeaderMap) -> bool {
    let accepted = |expected: &str| {
        headers
            .get_all(header::ACCEPT)
            .iter()
            .filter_map(|v| v.to_str().ok())
            .flat_map(|v| v.split(','))
            .any(|part| {
                let mut fields = part.trim().split(';');
                fields
                    .next()
                    .is_some_and(|v| v.trim().eq_ignore_ascii_case(expected))
                    && !fields.any(|v| {
                        v.trim()
                            .strip_prefix("q=")
                            .and_then(|q| q.parse::<f32>().ok())
                            .is_some_and(|q| q <= 0.0)
                    })
            })
    };
    accepted("application/json") && accepted("text/event-stream")
}

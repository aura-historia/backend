use serde_json::json;
use std::{
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    process::{Command, Stdio},
    time::{Duration, Instant},
};

#[test]
fn should_keep_protocol_payloads_out_of_binary_logs_even_with_verbose_environment() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    drop(listener);
    let mut child = Command::new(env!("CARGO_BIN_EXE_aura-historia-mcp"))
        .env_clear()
        .env("POSTGRES_HOST", "127.0.0.1")
        .env("POSTGRES_PORT", "9")
        .env("POSTGRES_DATABASE", "fixture")
        .env("POSTGRES_USERNAME", "fixture")
        .env("POSTGRES_PASSWORD", "fixture-password")
        .env("POSTGRES_TLS_ROOT_CERT", "/fixture/ca.pem")
        .env("STAGE", "ephemeral")
        .env(
            "PRODUCT_LISTING_INGESTION_QUEUE_URL",
            "http://127.0.0.1:9/fixture.fifo",
        )
        .env("AWS_REGION", "eu-central-1")
        .env("AWS_ACCESS_KEY_ID", "fixture")
        .env("AWS_SECRET_ACCESS_KEY", "fixture")
        .env("AWS_EC2_METADATA_DISABLED", "true")
        .env("OAUTH_ISSUER_URL", "https://api.stage.aura-historia.com")
        .env("OPENSEARCH_ENDPOINT_URL", "http://127.0.0.1:9")
        .env("MCP_BIND_ADDR", address.to_string())
        .env("MCP_ALLOWED_HOSTS", address.to_string())
        .env_remove("MCP_ALLOWED_ORIGINS")
        .env("RUST_LOG", "trace")
        .env("LOG_LEVEL", "TRACE")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let started = Instant::now();
    let mut stream = loop {
        match TcpStream::connect(address) {
            Ok(stream) => break stream,
            Err(_) if started.elapsed() < Duration::from_secs(5) => {
                std::thread::sleep(Duration::from_millis(10))
            }
            Err(error) => {
                let _ = child.kill();
                panic!("native binary did not accept connections: {error}")
            }
        }
    };
    stream
        .set_read_timeout(Some(Duration::from_secs(3)))
        .unwrap();
    let body=json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"missingOperation","arguments":{"secret":"PRIVATE_ARGUMENT_SENTINEL"},"_meta":{
        "io.modelcontextprotocol/protocolVersion":"2026-07-28","io.modelcontextprotocol/clientInfo":{"name":"logging-test","version":"1"},"io.modelcontextprotocol/clientCapabilities":{}
    }}}).to_string();
    let request = format!(
        "POST /v1 HTTP/1.1\r\nHost: {address}\r\nContent-Type: application/json\r\nAccept: application/json, text/event-stream\r\nMcp-Protocol-Version: 2026-07-28\r\nMcp-Method: tools/call\r\nMcp-Name: missingOperation\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(request.as_bytes()).unwrap();
    let mut response = String::new();
    stream.read_to_string(&mut response).unwrap();
    assert!(response.starts_with("HTTP/1.1 200"));
    assert!(response.contains("Unknown tool"));
    let mut stream = TcpStream::connect(address).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(3)))
        .unwrap();
    let rejected = request.replace(
        "Content-Type:",
        "Authorization: Bearer PRIVATE_BEARER_SENTINEL\r\nContent-Type:",
    );
    stream.write_all(rejected.as_bytes()).unwrap();
    let mut response = String::new();
    stream.read_to_string(&mut response).unwrap();
    assert!(response.starts_with("HTTP/1.1 401"));
    assert!(!response.contains("PRIVATE_BEARER_SENTINEL"));
    assert!(!response.contains("PRIVATE_ARGUMENT_SENTINEL"));
    let _ = child.kill();
    let output = child.wait_with_output().unwrap();
    let logs = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!logs.contains("PRIVATE_ARGUMENT_SENTINEL"));
    assert!(!logs.contains("PRIVATE_BEARER_SENTINEL"));
    assert!(!logs.contains("missingOperation"));
}

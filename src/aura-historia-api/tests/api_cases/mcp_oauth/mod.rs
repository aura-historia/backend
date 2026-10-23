//! Generic fixed-client acceptance over real local OAuth, REST and MCP HTTP boundaries.
mod client;
mod mcp;

use crate::{AURA_API, BUSINESS_SCHEMA, OPENSEARCH, api_support};
use client::{CALLBACK, GenericClient, MCP_RESOURCE};
use mcp::{McpServer, successful_result};
use reqwest::StatusCode;
use serde_json::{Value, json};
use test_api::{IntegrationTestService, aura_integration_test, get_postgres_client};

// The production composition reads provider configuration. Isolate that configuration in a
// child process; selected overview/account calls never contact this deliberately closed port.
#[test]
fn should_verify_generic_oauth_and_real_hybrid_mcp() {
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["mcp_oauth::cases::", "--ignored", "--nocapture"])
        // This parent owns no lease; the child batch allocates its own bounded contexts.
        .env_remove("NEXTEST")
        .env_remove("NEXTEST_RUN_ID")
        .env_remove("NEXTEST_TEST_GLOBAL_SLOT")
        .env_remove("NEXTEST_EXECUTION_MODE")
        .env("STAGE", "ephemeral")
        .env("OPENSEARCH_ENDPOINT_URL", "http://127.0.0.1:9")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("11 passed; 0 failed"));
    for line in String::from_utf8_lossy(&output.stdout).lines() {
        if line.starts_with("test result:")
            || (line.starts_with("test mcp_oauth::cases::") && line.ends_with(" ... ok"))
        {
            println!("{line}");
        }
    }
}

mod cases;

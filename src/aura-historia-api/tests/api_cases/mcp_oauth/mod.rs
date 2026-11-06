//! Generic acceptance over real local OAuth, REST and MCP HTTP boundaries, for fixed
//! registered clients and for public clients identified by a metadata document URL.
mod client;
mod mcp;

use crate::{AURA_API, BUSINESS_SCHEMA, OPENSEARCH, api_support};
use client::{GenericClient, MCP_RESOURCE};
use mcp::{McpServer, successful_result};
use reqwest::StatusCode;
use serde_json::{Value, json};
use test_api::{IntegrationTestService, aura_integration_test, get_postgres_client};

// The production composition reads provider configuration. Isolate that configuration in a
// child process; selected overview/account calls never contact this deliberately closed port.
#[test]
fn should_verify_generic_oauth_and_real_hybrid_mcp() {
    run_isolated_cases("mcp_oauth::cases::", 10);
}

#[test]
fn should_verify_metadata_document_clients_and_real_hybrid_mcp() {
    run_isolated_cases("mcp_oauth::metadata_document_cases::", 3);
}

fn run_isolated_cases(filter: &str, expected_passed: usize) {
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args([filter, "--ignored", "--nocapture"])
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
    assert!(
        String::from_utf8_lossy(&output.stdout)
            .contains(&format!("{expected_passed} passed; 0 failed"))
    );
    for line in String::from_utf8_lossy(&output.stdout).lines() {
        if line.starts_with("test result:")
            || (line.starts_with(&format!("test {filter}")) && line.ends_with(" ... ok"))
        {
            println!("{line}");
        }
    }
}

mod cases;
mod metadata_document_cases;

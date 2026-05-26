use anyhow::{Context, Result};
use camino::Utf8Path;
use determinator::Determinator;
use guppy::{MetadataCommand, graph::DependencyDirection};
use std::collections::HashSet;
use std::process::Command;

/// Integration test crates that run on ubuntu-latest with LocalStack.
/// These paths are relative to the workspace root.
const INTEGRATION_TEST_CRATES: &[&str] = &[
    "src/crawler",
    "src/fxrate",
    "src/notification",
    "src/notification-api",
    "src/partner-shop-application",
    "src/partner-shop-application-api",
    "src/partner-shop-application-lambda",
    "src/product",
    "src/product-api",
    "src/product-api-partner",
    "src/product-lambda/src/product-lambda-ingest-partner-products",
    "src/product-pipeline/src/product-pipeline-embed-text",
    "src/product-pipeline/src/product-pipeline-translate",
    "src/product-watchlist",
    "src/product-watchlist-api",
    "src/search-filter",
    "src/search-filter-api",
    "src/search-filter-lambda/src/search-filter-lambda-periodic-match",
    "src/search-filter-lambda/src/search-filter-lambda-percolate-product",
    "src/shop",
    "src/shop-api",
    "src/shop-lambda/src/shop-lambda-opensearch-index",
    "src/shopify-lambda",
    "src/test-api",
    "src/user",
    "src/user-api",
    "src/user-lambda/src/user-lambda-index-opensearch",
    "src/user-lambda/src/user-lambda-tier-update",
    "src/webhook-api",
];

/// Acceptance test crates that run on self-hosted runners with cargo-lambda.
/// These paths are relative to the workspace root.
const ACCEPTANCE_TEST_CRATES: &[&str] = &["src/acceptance-tests"];

fn main() -> Result<()> {
    let base_ref = std::env::args()
        .nth(1)
        .context("Usage: ci-determinator <base-git-ref>")?;

    let changed_files = get_changed_files(&base_ref)?;

    if changed_files.is_empty() {
        let output = serde_json::json!({
            "integration_test": Vec::<&str>::new(),
            "acceptance_test": Vec::<&str>::new(),
        });
        println!("{output}");
        return Ok(());
    }

    let graph = MetadataCommand::new()
        .exec()
        .context("Failed to run cargo metadata")?
        .build_graph()
        .context("Failed to build package graph")?;

    let mut determinator = Determinator::new(&graph, &graph);
    determinator.add_changed_paths(changed_files.iter().map(String::as_str));
    let result = determinator.compute();

    let workspace_root = graph.workspace().root();
    let affected_dirs: HashSet<String> = result
        .affected_set
        .packages(DependencyDirection::Forward)
        .filter(|p| p.in_workspace())
        .filter_map(|p| {
            let rel = p.manifest_path().strip_prefix(workspace_root).ok()?;
            let dir = rel.parent()?;
            Some(dir.to_string())
        })
        .collect();

    let integration_test: Vec<&str> = INTEGRATION_TEST_CRATES
        .iter()
        .copied()
        .filter(|c| affected_dirs.contains(*c))
        .collect();

    let acceptance_test: Vec<&str> = ACCEPTANCE_TEST_CRATES
        .iter()
        .copied()
        .filter(|c| affected_dirs.contains(*c))
        .collect();

    let output = serde_json::json!({
        "integration_test": integration_test,
        "acceptance_test": acceptance_test,
    });
    println!("{output}");

    Ok(())
}

fn get_changed_files(base_ref: &str) -> Result<Vec<String>> {
    let output = Command::new("git")
        .args(["diff", "--name-only", base_ref])
        .output()
        .context("Failed to run git diff")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("git diff failed: {stderr}");
    }

    let stdout = String::from_utf8(output.stdout).context("git diff output is not valid UTF-8")?;
    let files = stdout
        .lines()
        .filter(|l| !l.is_empty())
        .map(|l| Utf8Path::new(l).to_string())
        .collect();

    Ok(files)
}

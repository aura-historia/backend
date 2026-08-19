use anyhow::{Context, Result};
use camino::Utf8Path;
use determinator::Determinator;
use guppy::{MetadataCommand, graph::DependencyDirection};
use std::collections::HashSet;
use std::process::Command;

/// Integration test crates that run on ubuntu-latest with LocalStack.
/// These paths are relative to the workspace root.
const INTEGRATION_TEST_CRATES: &[&str] = &[
    "src/aura-historia-api",
    "src/aura-historia-worker",
    "src/common",
    "src/crawler",
    "src/embedding",
    "src/geo",
    "src/image-fetcher",
    "src/large-language-model",
    "src/product-core",
    "src/product-opensearch",
    "src/product-postgres",
    "src/product-service",
    "src/shop-core",
    "src/shop-postgres",
    "src/shop-service",
    "src/shop-partner-core",
    "src/shop-partner-postgres",
    "src/shop-partner-service",
    "src/user-core",
    "src/user-dynamodb",
    "src/user-postgres",
    "src/user-service",
    "src/user-zoho",
    "src/search-filter-core",
    "src/search-filter-opensearch",
    "src/search-filter-postgres",
    "src/search-filter-service",
    "src/watchlist-core",
    "src/watchlist-postgres",
    "src/watchlist-service",
    "src/notification-core",
    "src/notification-email-aws",
    "src/notification-postgres",
    "src/notification-service",
    "src/oauth-core",
    "src/oauth-dynamodb",
    "src/oauth-service",
    "src/billing-service",
    "src/billing-stripe",
    "src/stripe-lambda",
    "src/shopify-lambda",
    "src/fxrate-lambda",
    "src/fxrate-core",
    "src/fxrate-postgres",
    "src/fxrate-service",
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

    let infrastructure_changed = changed_files.iter().any(|path| {
        path.starts_with("infra/")
            || path == ".github/workflows/deploy.yml"
            || path == ".github/workflows/integrate.yml"
    });

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

    let acceptance_test: Vec<&str> = if infrastructure_changed {
        ACCEPTANCE_TEST_CRATES.to_vec()
    } else {
        ACCEPTANCE_TEST_CRATES
            .iter()
            .copied()
            .filter(|c| affected_dirs.contains(*c))
            .collect()
    };

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

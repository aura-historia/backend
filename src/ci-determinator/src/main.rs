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

const NOTIFICATION_DELIVERY_TEST_CRATES: &[&str] = &[
    "src/notification-core",
    "src/notification-email-aws",
    "src/notification-postgres",
    "src/notification-service",
    "src/aura-historia-worker",
];

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

    let infrastructure_changed = has_infrastructure_change(&changed_files);
    let notification_delivery_changed = has_notification_delivery_change(&changed_files);

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

    let mut integration_test = integration_test_crates(&affected_dirs);
    if notification_delivery_changed {
        for crate_path in NOTIFICATION_DELIVERY_TEST_CRATES {
            if !integration_test.contains(crate_path) {
                integration_test.push(crate_path);
            }
        }
    }

    let acceptance_test: Vec<&str> = if infrastructure_changed || notification_delivery_changed {
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

fn integration_test_crates(affected_dirs: &HashSet<String>) -> Vec<&'static str> {
    INTEGRATION_TEST_CRATES
        .iter()
        .copied()
        .filter(|crate_path| affected_dirs.contains(*crate_path))
        .collect()
}

fn has_infrastructure_change(changed_files: &[String]) -> bool {
    changed_files.iter().any(|path| {
        path.starts_with("infra/")
            || path.starts_with("mjml/")
            || path == ".github/workflows/deploy.yml"
            || path == ".github/workflows/integrate.yml"
    })
}

fn has_notification_delivery_change(changed_files: &[String]) -> bool {
    changed_files.iter().any(|path| {
        path.starts_with("migrations/")
            || path.starts_with("src/notification-core/")
            || path.starts_with("src/notification-service/")
            || path.starts_with("src/notification-postgres/")
            || path.starts_with("src/notification-email-aws/")
            || matches!(
                path.as_str(),
                "src/aura-historia-worker/src/cdc.rs"
                    | "src/aura-historia-worker/src/notification_delivery.rs"
                    | "src/aura-historia-worker/src/main.rs"
                    | "src/aura-historia-worker/src/lib.rs"
            )
    })
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_catalog_all_notification_delivery_packages() {
        assert!(
            NOTIFICATION_DELIVERY_TEST_CRATES
                .iter()
                .all(|crate_path| INTEGRATION_TEST_CRATES.contains(crate_path))
        );
    }

    #[test]
    fn should_select_notification_delivery_validation_for_migrations_and_runtime_changes() {
        for changed_path in [
            "migrations/20260725090000_initial_business_schema.sql",
            "src/notification-core/src/notification_delivery.rs",
            "src/notification-service/src/notification_creation.rs",
            "src/notification-postgres/src/delivery_repository.rs",
            "src/notification-email-aws/src/sender.rs",
            "src/aura-historia-worker/src/cdc.rs",
            "src/aura-historia-worker/src/notification_delivery.rs",
        ] {
            assert!(has_notification_delivery_change(&[changed_path.to_owned()]));
        }
    }

    #[test]
    fn should_select_acceptance_for_worker_infrastructure_and_sequin_related_changes() {
        for changed_path in [
            "infra/src/application-stack.ts",
            "mjml/notification.mjml",
            ".github/workflows/deploy.yml",
            "src/aura-historia-worker/src/main.rs",
        ] {
            let changed_files = vec![changed_path.to_owned()];
            assert!(
                has_infrastructure_change(&changed_files)
                    || has_notification_delivery_change(&changed_files)
            );
        }
    }

    #[test]
    fn should_select_affected_integration_crates() {
        let affected_dirs = HashSet::from([
            "src/notification-email-aws".to_owned(),
            "src/aura-historia-worker".to_owned(),
        ]);

        assert_eq!(
            vec!["src/aura-historia-worker", "src/notification-email-aws"],
            integration_test_crates(&affected_dirs)
        );
    }
}

use anyhow::{Context, Result};
use camino::Utf8Path;
use determinator::Determinator;
use guppy::{MetadataCommand, graph::DependencyDirection};
use std::collections::BTreeSet;
use std::process::Command;

fn main() -> Result<()> {
    let base_ref = std::env::args()
        .nth(1)
        .context("Usage: ci-determinator <base-git-ref>")?;

    let changed_files = get_changed_files(&base_ref)?;

    if changed_files.is_empty() {
        let output = serde_json::json!({
            "integration_test": Vec::<&str>::new(),

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
    let integration_test: BTreeSet<String> = result
        .affected_set
        .packages(DependencyDirection::Forward)
        .filter(|package| package.in_workspace())
        .filter_map(|package| {
            relative_workspace_package_dir(package.manifest_path(), workspace_root)
        })
        .filter(|package_dir| should_run_integration_test(package_dir))
        .collect();

    let output = serde_json::json!({
        "integration_test": integration_test,

    });
    println!("{output}");

    Ok(())
}

fn relative_workspace_package_dir(
    manifest_path: &Utf8Path,
    workspace_root: &Utf8Path,
) -> Option<String> {
    let relative_manifest = manifest_path.strip_prefix(workspace_root).ok()?;
    let directory = relative_manifest.parent()?;

    Some(if directory.as_str().is_empty() {
        ".".to_owned()
    } else {
        directory.to_string()
    })
}

fn should_run_integration_test(package_dir: &str) -> bool {
    !matches!(package_dir, "." | "src/test-api/src/test-api-macros")
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
    use super::{relative_workspace_package_dir, should_run_integration_test};
    use camino::Utf8Path;

    #[test]
    fn should_exclude_workspace_root_package() {
        let package_dir = relative_workspace_package_dir(
            Utf8Path::new("/workspace/Cargo.toml"),
            Utf8Path::new("/workspace"),
        )
        .expect("workspace root package should have a path");

        assert!(!should_run_integration_test(&package_dir));
    }

    #[test]
    fn should_exclude_test_api_macros_package() {
        let package_dir = relative_workspace_package_dir(
            Utf8Path::new("/workspace/src/test-api/src/test-api-macros/Cargo.toml"),
            Utf8Path::new("/workspace"),
        )
        .expect("nested package should have a path");

        assert!(!should_run_integration_test(&package_dir));
    }

    #[test]
    fn should_include_regular_workspace_package() {
        assert!(should_run_integration_test("src/test-api"));
    }
}

use super::contract::Profile;
use super::{Result, invalid, services, supervisor};
use std::ffi::OsString;
use std::path::PathBuf;

const USAGE: &str = "usage: aura-test-env run --profile postgres|aws|opensearch|api --slots 1..1024 [--state-dir ABSOLUTE] -- COMMAND [ARG...] | gc [--state-dir ABSOLUTE] | probe";

/// Run the developer CLI; stdout/stderr of a supervised command are inherited.
pub async fn run_cli(args: Vec<OsString>) -> Result<i32> {
    let Some(mode) = args.first().and_then(|s| s.to_str()) else {
        return Err(invalid(USAGE));
    };
    if mode == "probe" && args.len() == 1 {
        let config = super::managed_environment()
            .ok_or_else(|| invalid("probe requires a managed environment"))?;
        services::probe(config).await?;
        return Ok(0);
    }
    let mut seen = std::collections::BTreeSet::new();
    let mut root = supervisor::default_state_dir();
    let mut profile = None;
    let mut slots = 1;
    let mut i = 1;
    while i < args.len() && args[i] != "--" {
        let key = args[i].to_str().ok_or_else(|| invalid(USAGE))?;
        if !seen.insert(key) {
            return Err(invalid("duplicate supervisor option"));
        }
        let value = args.get(i + 1).ok_or_else(|| invalid(USAGE))?;
        match key {
            "--state-dir" => root = PathBuf::from(value),
            "--slots" if mode == "run" => {
                slots = value.to_str().ok_or_else(|| invalid(USAGE))?.parse()?
            }
            "--profile" if mode == "run" => {
                profile = Some(match value.to_str() {
                    Some("postgres") => Profile::Postgres,
                    Some("aws") => Profile::Aws,
                    Some("opensearch") => Profile::Opensearch,
                    Some("api") => Profile::Api,
                    _ => return Err(invalid(USAGE)),
                })
            }
            _ => return Err(invalid(USAGE)),
        }
        i += 2;
    }
    match mode {
        "gc" if i == args.len() && root.is_absolute() => {
            supervisor::gc(&root).await?;
            Ok(0)
        }
        "run" if i + 1 < args.len() && args[i] == "--" => {
            supervisor::run(
                profile.ok_or_else(|| invalid(USAGE))?,
                slots,
                &root,
                &args[i + 1..],
            )
            .await
        }
        _ => Err(invalid(USAGE)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn should_reject_invalid_cli_before_allocating_resources() {
        for args in [
            vec![],
            vec!["run", "--profile", "unknown", "--", "true"],
            vec!["run", "--profile", "aws", "--slots", "0", "--", "true"],
            vec![
                "run",
                "--profile",
                "aws",
                "--profile",
                "postgres",
                "--",
                "true",
            ],
            vec!["run", "--profile", "aws", "--"],
            vec!["gc", "--state-dir", "relative"],
        ] {
            assert!(
                run_cli(args.into_iter().map(OsString::from).collect())
                    .await
                    .is_err()
            );
        }
    }
}

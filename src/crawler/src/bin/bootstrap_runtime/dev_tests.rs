use super::*;
use rstest::rstest;
use std::os::unix::ffi::OsStringExt;

const CA_FILE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/fixtures/crawler-postgres-ca.pem"
);
const DEV_URL: &str = "postgres://private_user:explicit_password@dev-db.invalid:6432/private_db";

fn inputs() -> BTreeMap<&'static str, String> {
    let mut values = environment(DEV_URL);
    values.insert("STAGE", "dev".into());
    values.insert("POSTGRES_SSL_MODE", "verify-full".into());
    values.insert("POSTGRES_SSL_ROOT_CERT", CA_FILE.into());
    values
}

fn url_keys(target: Target) -> (&'static str, &'static str) {
    match target {
        Target::Business => ("BUSINESS_DATABASE_URL", "LOCAL_DB_URL"),
        Target::Crawler => ("LOCAL_DB_URL", "BUSINESS_DATABASE_URL"),
    }
}

fn dev_load(
    target: Target,
    initialize: bool,
    values: &BTreeMap<&'static str, String>,
) -> Result<PostgresPoolConfig, Failure> {
    config::load(Entrypoint::Dev, target, initialize, |key| {
        values.get(key).cloned().ok_or(VarError::NotPresent)
    })
}

#[rstest]
fn should_accept_only_explicit_dev_actions(
    #[values("business", "crawler")] target: &str,
    #[values("--initialize-fresh", "--verify")] action: &str,
) -> TestResult {
    let target_value = match target {
        "business" => Target::Business,
        _ => Target::Crawler,
    };
    let command = parse(Entrypoint::Dev, [action, target].map(OsString::from))?;
    assert_eq!(
        command,
        if action == "--initialize-fresh" {
            Command::Initialize(target_value)
        } else {
            Command::Verify(target_value)
        }
    );
    Ok(())
}

#[rstest]
#[case(vec![])]
#[case(vec!["--legacy"])]
#[case(vec!["business"])]
#[case(vec!["--initialize-fresh"])]
#[case(vec!["--verify"])]
#[case(vec!["--initialize-fresh", "both"])]
#[case(vec!["--verify", "BUSINESS"])]
#[case(vec!["--initialize-fresh", "crawler", SECRET])]
#[case(vec!["--verify", "business", "--initialize-fresh", "crawler"])]
#[case(vec!["--verify=business"])]
#[case(vec!["--initialize-fresh=business"])]
#[case(vec!["--help", SECRET])]
#[case(vec!["--"])]
#[case(vec![SECRET])]
fn should_refuse_dev_legacy_and_bad_cli_before_config_or_dependencies(
    #[case] args: Vec<&str>,
) -> TestResult {
    let mut writes = false;
    let error = dispatch(
        Entrypoint::Dev,
        args.into_iter().map(OsString::from),
        &mut writes,
    )
    .err()
    .ok_or("accepted invalid dev CLI")?;
    assert_eq!(error.code, Code::Usage);
    assert_eq!(error.code.exit(), 2);
    safe_chain(&error);
    assert!(!writes);
    Ok(())
}

#[test]
fn should_refuse_nonunicode_dev_cli_before_config_or_dependencies() -> TestResult {
    let invalid = OsString::from_vec(vec![0xff]);
    for args in [
        vec![invalid.clone()],
        vec!["--initialize-fresh".into(), invalid.clone()],
        vec!["--verify".into(), invalid.clone()],
        vec!["--verify".into(), "crawler".into(), invalid],
    ] {
        let mut writes = false;
        let error = dispatch(Entrypoint::Dev, args, &mut writes)
            .err()
            .ok_or("accepted nonunicode dev CLI")?;
        assert_eq!(error.code, Code::Usage);
        safe_chain(&error);
        assert!(!writes);
    }
    Ok(())
}

#[test]
fn should_show_dev_help_without_legacy_fallback_or_configuration() -> TestResult {
    let mut writes = false;
    let help = dispatch(Entrypoint::Dev, ["--help".into()], &mut writes)?;
    assert_eq!(help, DEV_HELP);
    assert!(help.contains("STAGE=dev"));
    assert!(help.contains("POSTGRES_SSL_MODE=verify-full"));
    assert!(!help.contains("bootstrap-local"));
    assert!(!writes);
    assert_eq!(parse(Entrypoint::Local, [])?, Command::Legacy);
    Ok(())
}

#[rstest]
fn should_accept_remote_dev_tls_and_read_only_selected_url(
    #[values(Target::Business, Target::Crawler)] target: Target,
    #[values(false, true)] initialize: bool,
    #[values(false, true)] invalid_other: bool,
) -> TestResult {
    let values = inputs();
    let (selected, other) = url_keys(target);
    let mut read = Vec::new();
    let config = config::load(Entrypoint::Dev, target, initialize, |key| {
        read.push(key);
        if key == other {
            if invalid_other {
                Err(VarError::NotUnicode(OsString::from_vec(vec![0xff])))
            } else {
                Err(VarError::NotPresent)
            }
        } else {
            values.get(key).cloned().ok_or(VarError::NotPresent)
        }
    })?;
    assert!(read.contains(&selected));
    assert!(!read.contains(&other));
    assert_eq!(read.iter().filter(|key| **key == "STAGE").count(), 1);
    assert_eq!(config.host(), "dev-db.invalid");
    assert_eq!(config.port(), 6432);
    assert_eq!(config.database(), "private_db");
    assert_eq!(config.max_connections(), 1);
    assert!(matches!(
        config.connect_options().get_ssl_mode(),
        sqlx::postgres::PgSslMode::VerifyFull
    ));
    assert_eq!(
        config.connect_options().get_application_name(),
        Some("crawler-bootstrap-dev")
    );
    assert!(!format!("{config:?} {config:#?}").contains("private_"));
    Ok(())
}

#[rstest]
fn should_refuse_every_stage_except_exact_dev_before_tls_or_url_lookup(
    #[values(Target::Business, Target::Crawler)] target: Target,
    #[values(false, true)] initialize: bool,
    #[values(
        None,
        Some(""),
        Some("prod"),
        Some("local"),
        Some("ephemeral"),
        Some("test"),
        Some("DEV"),
        Some(" dev"),
        Some("dev "),
        Some("unknown")
    )]
    stage: Option<&str>,
) -> TestResult {
    let mut read = Vec::new();
    let error = config::load(Entrypoint::Dev, target, initialize, |key| {
        read.push(key);
        stage.map(str::to_owned).ok_or(VarError::NotPresent)
    })
    .err()
    .ok_or("accepted unsupported dev stage")?;
    assert_eq!(read, ["STAGE"]);
    assert_eq!(
        error.code,
        if stage.is_none() {
            Code::Config
        } else {
            Code::UnsupportedStage
        }
    );
    safe_chain(&error);
    Ok(())
}

#[rstest]
#[case("POSTGRES_SSL_MODE", None)]
#[case("POSTGRES_SSL_MODE", Some(""))]
#[case("POSTGRES_SSL_MODE", Some("disable"))]
#[case("POSTGRES_SSL_MODE", Some("prefer"))]
#[case("POSTGRES_SSL_MODE", Some("require"))]
#[case("POSTGRES_SSL_MODE", Some("verify-ca"))]
#[case("POSTGRES_SSL_ROOT_CERT", None)]
#[case("POSTGRES_SSL_ROOT_CERT", Some(""))]
#[case(
    "POSTGRES_SSL_ROOT_CERT",
    Some("/private/sentinel_password_provider_body_private_path")
)]
#[case("POSTGRES_SSL_ROOT_CERT", Some(concat!(env!("CARGO_MANIFEST_DIR"), "/Cargo.toml")))]
#[case("PGSSLCERT", Some(""))]
#[case("PGSSLKEY", Some(SECRET))]
#[case("PGSSLROOTCERT", Some(SECRET))]
#[case("PGOPTIONS", Some(SECRET))]
fn should_refuse_missing_or_unsafe_dev_tls_with_redacted_errors(
    #[values(Target::Business, Target::Crawler)] target: Target,
    #[values(false, true)] initialize: bool,
    #[case] key: &'static str,
    #[case] value: Option<&str>,
) -> TestResult {
    let mut values = inputs();
    values.remove(key);
    if let Some(value) = value {
        values.insert(key, value.into());
    }
    let error = dev_load(target, initialize, &values)
        .err()
        .ok_or("accepted unsafe dev TLS")?;
    assert_eq!(error.code, Code::Config);
    safe_chain(&error);
    Ok(())
}

#[rstest]
fn should_refuse_missing_or_invalid_selected_dev_url_without_other_url_fallback(
    #[values(Target::Business, Target::Crawler)] target: Target,
    #[values(false, true)] initialize: bool,
    #[values(
        None,
        Some(""),
        Some(SECRET),
        Some("postgres://dev-db.invalid/private_db"),
        Some("postgres://private_user@dev-db.invalid/private_db"),
        Some("postgres://private_user:password@dev-db.invalid/")
    )]
    url: Option<&str>,
) -> TestResult {
    let mut values = inputs();
    let (selected, _) = url_keys(target);
    values.remove(selected);
    if let Some(url) = url {
        values.insert(selected, url.into());
    }
    let error = dev_load(target, initialize, &values)
        .err()
        .ok_or("accepted absent or invalid selected dev URL")?;
    assert_eq!(error.code, Code::Config);
    safe_chain(&error);
    Ok(())
}

#[rstest]
fn should_refuse_dev_url_tls_downgrades_and_overrides(
    #[values(Target::Business, Target::Crawler)] target: Target,
    #[values(false, true)] initialize: bool,
    #[values(
        "?sslmode=disable",
        "?sslmode=prefer",
        "?sslmode=require",
        "?sslmode=verify-ca",
        "?sslrootcert=private",
        "?application_name=crawler-bootstrap-local",
        "?host=localhost",
        "?options=private",
        "?sslmode=verify-full&sslmode=verify-full"
    )]
    suffix: &str,
) -> TestResult {
    let mut values = inputs();
    values.insert(url_keys(target).0, format!("{DEV_URL}{suffix}"));
    let error = dev_load(target, initialize, &values)
        .err()
        .ok_or("accepted dev URL policy override")?;
    assert_eq!(error.code, Code::Config);
    safe_chain(&error);
    Ok(())
}

#[rstest]
fn should_accept_matching_dev_url_policy_without_weakening_local_stage_gate(
    #[values(Target::Business, Target::Crawler)] target: Target,
    #[values(false, true)] initialize: bool,
) -> TestResult {
    let mut values = inputs();
    values.insert(
        url_keys(target).0,
        format!("{DEV_URL}?sslmode=verify-full&application_name=crawler-bootstrap-dev"),
    );
    dev_load(target, initialize, &values)?;
    let error = load(target, initialize, &values)
        .err()
        .ok_or("local executable accepted dev stage")?;
    assert_eq!(error.code, Code::UnsupportedStage);
    Ok(())
}

#[rstest]
fn should_refuse_nonunicode_selected_dev_environment_without_lossy_conversion(
    #[values(Target::Business, Target::Crawler)] target: Target,
    #[values(false, true)] initialize: bool,
    #[values(
        "STAGE",
        "POSTGRES_SSL_MODE",
        "POSTGRES_SSL_ROOT_CERT",
        "SELECTED_URL",
        "PGSSLCERT",
        "PGSSLKEY",
        "PGSSLROOTCERT",
        "PGOPTIONS"
    )]
    key: &str,
) -> TestResult {
    let values = inputs();
    let key = if key == "SELECTED_URL" {
        url_keys(target).0
    } else {
        key
    };
    let error = config::load(Entrypoint::Dev, target, initialize, |name| {
        if name == key {
            Err(VarError::NotUnicode(OsString::from_vec(vec![0xff])))
        } else {
            values.get(name).cloned().ok_or(VarError::NotPresent)
        }
    })
    .err()
    .ok_or("accepted nonunicode dev input")?;
    assert_eq!(error.code, Code::Config);
    safe_chain(&error);
    Ok(())
}

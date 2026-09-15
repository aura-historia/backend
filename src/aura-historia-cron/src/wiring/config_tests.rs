use super::*;
use std::{collections::BTreeMap, ffi::OsString};

type TestResult = Result<(), Box<dyn Error>>;
const CA: &str = "OPENSEARCH_SSL_ROOT_CERT";
const PUBLIC_CA: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/src/postgres-test-ca.crt");
const SCHEDULE: &str = "SEARCH_FILTER_PERIODIC_MATCH_CRON";
const NUMBERS: &[&str] = &[
    "PERIODIC_MATCH_FILTER_PAGE_SIZE",
    "PERIODIC_MATCH_HYBRID_SCAN_LIMIT",
    "PERIODIC_MATCH_EVALUATION_LIMIT",
    "PERIODIC_MATCH_LLM_CONCURRENCY",
    "PERIODIC_MATCH_MAX_ATTEMPTS",
    "PERIODIC_MATCH_MAX_RUN_SECONDS",
    "PERIODIC_MATCH_PROJECTION_LAG_SECONDS",
    "PERIODIC_MATCH_REPLAY_OVERLAP_SECONDS",
];

fn inputs() -> BTreeMap<&'static str, OsString> {
    [
        ("STAGE", "test"),
        ("POSTGRES_SSL_MODE", "disable"),
        ("POSTGRES_HOST", "postgres.example.test"),
        ("POSTGRES_DATABASE", "database_canary"),
        ("POSTGRES_USERNAME", "username_canary"),
        ("POSTGRES_PASSWORD", "password_canary"),
        ("OPENSEARCH_ENDPOINT_URL", "https://opensearch.example.test"),
        ("VERTEX_AI_PROJECT_ID", "project_canary"),
        ("VERTEX_AI_LOCATION", "location_canary"),
        ("VERTEX_AI_MODEL", "model_canary"),
    ]
    .map(|(name, value)| (name, value.into()))
    .into()
}

fn parse(values: &BTreeMap<&'static str, OsString>) -> Result<PeriodicMatchConfig, WiringError> {
    PeriodicMatchConfig::from_lookup(&mut |name| {
        values
            .get(name)
            .cloned()
            .ok_or(VarError::NotPresent)?
            .into_string()
            .map_err(VarError::NotUnicode)
    })
}

fn stage_inputs(stage: &str) -> BTreeMap<&'static str, OsString> {
    let mut values = inputs();
    values.insert("STAGE", stage.into());
    values.insert("OPENSEARCH_USERNAME", "username_canary".into());
    values.insert("OPENSEARCH_PASSWORD", "password_canary".into());
    if matches!(stage, "dev" | "prod") {
        values.insert("POSTGRES_SSL_MODE", "verify-full".into());
        values.insert("POSTGRES_SSL_ROOT_CERT", PUBLIC_CA.into());
    }
    values
}

struct CaFile(std::path::PathBuf);

impl CaFile {
    fn new() -> Result<Self, Box<dyn Error>> {
        use std::io::Write;
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "cron-ca-value_canary-{}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)?
                .as_nanos(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
        ));
        let mut file = std::fs::File::create_new(&path)?;
        let owned = Self(path);
        file.write_all(include_bytes!("../postgres-test-ca.crt"))?;
        Ok(owned)
    }
}

impl Drop for CaFile {
    fn drop(&mut self) {
        if let Err(error) = std::fs::remove_file(&self.0) {
            eprintln!("owned CA fixture cleanup failed: {:?}", error.kind());
        }
    }
}

#[test]
fn should_validate_opensearch_ca_for_real_stages_before_postgres_parsing() -> TestResult {
    let file = CaFile::new()?;
    for stage in ["dev", "prod"] {
        let mut values = stage_inputs(stage);
        // Invalid PG input is a tripwire: CA errors must win before even PG parsing.
        values.insert("POSTGRES_HOST", "".into());
        for (input, expected) in [
            (None, OpenSearchTlsError::MissingCa),
            (Some(""), OpenSearchTlsError::EmptyCaPath),
            (Some(" \t"), OpenSearchTlsError::EmptyCaPath),
            (
                Some("/missing/value_canary.pem"),
                OpenSearchTlsError::CaRead,
            ),
        ] {
            values.remove(CA);
            if let Some(value) = input {
                values.insert(CA, value.into());
            }
            let error = parse(&values).err().ok_or("invalid CA accepted")?;
            assert!(matches!(error, WiringError::OpenSearchTls(actual) if actual == expected));
            super::error_tests::assert_redacted_chain(&error);
            assert!(
                error
                    .source()
                    .is_some_and(|source| source.is::<OpenSearchTlsError>())
            );
        }
        values.insert(CA, file.0.clone().into_os_string());
        for bytes in [
            b"".as_slice(),
            b"value_canary",
            b"-----BEGIN CERTIFICATE-----\nvalue_canary\n-----END CERTIFICATE-----\n",
        ] {
            std::fs::write(&file.0, bytes)?;
            let error = parse(&values).err().ok_or("invalid PEM accepted")?;
            assert!(matches!(
                error,
                WiringError::OpenSearchTls(OpenSearchTlsError::InvalidCa)
            ));
            super::error_tests::assert_redacted_chain(&error);
        }
        values.insert(CA, PUBLIC_CA.into());
        values.insert("POSTGRES_HOST", "postgres.example.test".into());
        let config = parse(&values)?;
        assert!(config.auth.is_some());
        opensearch_client(&config)?;
        values.remove("OPENSEARCH_PASSWORD");
        assert!(matches!(
            parse(&values),
            Err(WiringError::MissingEnv {
                name: "OPENSEARCH_PASSWORD"
            })
        ));
    }
    Ok(())
}

#[cfg(unix)]
#[test]
fn should_reject_non_unicode_opensearch_ca_in_every_stage() -> TestResult {
    use std::os::unix::ffi::OsStringExt;
    for stage in ["dev", "prod", "local", "test", "ephemeral"] {
        let mut values = stage_inputs(stage);
        values.insert(CA, OsString::from_vec(b"value_canary\xff".to_vec()));
        let error = parse(&values).err().ok_or("non-Unicode CA accepted")?;
        assert!(matches!(
            error,
            WiringError::InvalidEnvEncoding { name: CA, .. }
        ));
        super::error_tests::assert_redacted_chain(&error);
        assert!(matches!(
            super::error_tests::original::<VarError>(&error)?,
            VarError::NotUnicode(_)
        ));
    }
    Ok(())
}

#[test]
fn should_reuse_frozen_opensearch_ca_when_building_runtime_clients() -> TestResult {
    let file = CaFile::new()?;
    let mut values = stage_inputs("prod");
    values.insert(CA, file.0.clone().into_os_string());
    let mut reads = 0;
    let config = PeriodicMatchConfig::from_lookup(&mut |name| {
        if name == CA {
            reads += 1;
        }
        values
            .get(name)
            .cloned()
            .ok_or(VarError::NotPresent)?
            .into_string()
            .map_err(VarError::NotUnicode)
    })?;
    assert_eq!(reads, 1);
    std::fs::write(&file.0, b"value_canary")?;
    opensearch_client(&config)?;
    opensearch_client(&config)?;
    assert!(parse(&values).is_err());
    let debug = format!("{:?}", config.tls.clone());
    assert!(!debug.contains("canary"));
    assert!(!debug.contains("BEGIN CERTIFICATE"));
    Ok(())
}

#[test]
fn should_keep_explicit_local_opensearch_policy_and_auth_semantics() -> TestResult {
    for stage in ["local", "test", "ephemeral"] {
        let mut values = stage_inputs(stage);
        for endpoint in ["https://localhost", "http://localhost"] {
            values.insert("OPENSEARCH_ENDPOINT_URL", endpoint.into());
            let config = parse(&values)?;
            assert!(config.auth.is_none());
            opensearch_client(&config)?;
        }
        values.insert(CA, PUBLIC_CA.into());
        assert!(matches!(
            parse(&values),
            Err(WiringError::OpenSearchTls(OpenSearchTlsError::CaWithHttp))
        ));
        values.insert("OPENSEARCH_ENDPOINT_URL", "https://localhost".into());
        opensearch_client(&parse(&values)?)?;
        values.insert(CA, "".into());
        assert!(matches!(
            parse(&values),
            Err(WiringError::OpenSearchTls(OpenSearchTlsError::EmptyCaPath))
        ));
    }
    Ok(())
}

#[test]
fn should_reject_unsafe_opensearch_endpoints_and_inexact_stages() -> TestResult {
    for stage in ["dev", "prod", "local", "test", "ephemeral"] {
        let mut values = stage_inputs(stage);
        values.insert(CA, PUBLIC_CA.into());
        for endpoint in [
            "https://username_canary:password_canary@localhost",
            "https://localhost?value_canary",
            "https://localhost#value_canary",
            "ftp://localhost",
            "value_canary",
        ] {
            values.insert("OPENSEARCH_ENDPOINT_URL", endpoint.into());
            let error = parse(&values).err().ok_or("unsafe endpoint accepted")?;
            assert!(matches!(
                error,
                WiringError::OpenSearchTls(_) | WiringError::OpenSearchUrl(_)
            ));
            super::error_tests::assert_redacted_chain(&error);
        }
        if matches!(stage, "dev" | "prod") {
            values.insert("OPENSEARCH_ENDPOINT_URL", "http://localhost".into());
            assert!(matches!(
                parse(&values),
                Err(WiringError::OpenSearchTls(
                    OpenSearchTlsError::HttpsRequired
                ))
            ));
        }
    }
    for stage in ["", "DEV", " dev", "test ", "value_canary"] {
        assert!(matches!(
            parse(&stage_inputs(stage)),
            Err(WiringError::OpenSearchTls(OpenSearchTlsError::InvalidStage))
        ));
    }
    let mut values = inputs();
    values.remove("STAGE");
    assert!(matches!(
        parse(&values),
        Err(WiringError::MissingEnv { name: "STAGE" })
    ));
    Ok(())
}

#[test]
fn should_use_job_defaults_only_when_inputs_are_absent() -> TestResult {
    let config = parse(&inputs())?;
    assert_eq!(config.schedule, "0 0 15 * * * *");
    assert_eq!(config.max_run_duration, Duration::from_secs(7200));
    assert_eq!(config.policy.filter_page_size.get(), 100);
    assert_eq!(config.policy.hybrid_scan_limit.get(), 100);
    assert_eq!(config.policy.evaluation_limit.get(), 50);
    assert_eq!(config.policy.llm_concurrency.get(), 8);
    assert_eq!(config.policy.max_attempts.get(), 3);
    assert_eq!(config.policy.projection_lag.whole_seconds(), 900);
    assert_eq!(config.policy.replay_overlap.whole_seconds(), 7200);
    Ok(())
}

#[test]
fn should_keep_job_input_trimming_and_zero_lag_support() -> TestResult {
    let mut values = inputs();
    values.insert(SCHEDULE, "  0 1 15 * * * *  ".into());
    for &name in NUMBERS {
        values.insert(name, " 1 ".into());
    }
    values.insert("PERIODIC_MATCH_PROJECTION_LAG_SECONDS", " 0 ".into());
    values.insert("PERIODIC_MATCH_REPLAY_OVERLAP_SECONDS", " 0 ".into());
    let config = parse(&values)?;
    assert_eq!(config.schedule, "0 1 15 * * * *");
    assert_eq!(config.max_run_duration, Duration::from_secs(1));
    assert_eq!(config.policy.filter_page_size.get(), 1);
    assert_eq!(config.policy.hybrid_scan_limit.get(), 1);
    assert_eq!(config.policy.evaluation_limit.get(), 1);
    assert_eq!(config.policy.llm_concurrency.get(), 1);
    assert_eq!(config.policy.max_attempts.get(), 1);
    assert_eq!(config.policy.projection_lag.whole_seconds(), 0);
    assert_eq!(config.policy.replay_overlap.whole_seconds(), 0);
    Ok(())
}

#[test]
fn should_reject_present_empty_and_malformed_job_inputs_during_config_parsing() -> TestResult {
    for name in std::iter::once(SCHEDULE).chain(NUMBERS.iter().copied()) {
        for value in ["", " \t\n ", "value_canary"] {
            let mut values = inputs();
            values.insert(name, value.into());
            let error = parse(&values).err().ok_or("invalid job input accepted")?;
            if name == SCHEDULE {
                assert!(matches!(error, WiringError::InvalidSchedule { .. }));
            } else {
                assert!(
                    matches!(error, WiringError::InvalidNumber { name: key, .. } if key == name)
                );
            }
            super::error_tests::assert_redacted_chain(&error);
        }
    }
    Ok(())
}

#[cfg(unix)]
#[test]
fn should_reject_non_unicode_job_inputs_and_retain_the_original_var_error() -> TestResult {
    use std::os::unix::ffi::OsStringExt;

    for name in std::iter::once(SCHEDULE).chain(NUMBERS.iter().copied()) {
        let mut values = inputs();
        let value = OsString::from_vec(b"value_canary\xff".to_vec());
        values.insert(name, value.clone());
        let error = parse(&values)
            .err()
            .ok_or("non-Unicode job input accepted")?;
        assert!(matches!(error, WiringError::InvalidEnvEncoding { name: key, .. } if key == name));
        super::error_tests::assert_redacted_chain(&error);
        let source = super::error_tests::original::<VarError>(&error)?;
        assert_eq!(source, &VarError::NotUnicode(value));
    }
    Ok(())
}

#[cfg(unix)]
#[test]
fn should_not_treat_non_unicode_required_inputs_as_missing() -> TestResult {
    use std::os::unix::ffi::OsStringExt;

    for name in [
        "STAGE",
        "OPENSEARCH_ENDPOINT_URL",
        "OPENSEARCH_SSL_ROOT_CERT",
        "OPENSEARCH_USERNAME",
        "OPENSEARCH_PASSWORD",
        "VERTEX_AI_PROJECT_ID",
        "VERTEX_AI_LOCATION",
        "VERTEX_AI_MODEL",
    ] {
        let mut values = inputs();
        values.insert("STAGE", "prod".into());
        values.insert("POSTGRES_SSL_MODE", "verify-full".into());
        values.insert(
            "POSTGRES_SSL_ROOT_CERT",
            concat!(env!("CARGO_MANIFEST_DIR"), "/src/postgres-test-ca.crt").into(),
        );
        values.insert(
            "OPENSEARCH_SSL_ROOT_CERT",
            concat!(env!("CARGO_MANIFEST_DIR"), "/src/postgres-test-ca.crt").into(),
        );
        values.insert("OPENSEARCH_USERNAME", "username_canary".into());
        values.insert("OPENSEARCH_PASSWORD", "password_canary".into());
        values.insert(name, OsString::from_vec(b"value_canary\xff".to_vec()));
        let error = parse(&values)
            .err()
            .ok_or("non-Unicode required input accepted")?;
        assert!(matches!(error, WiringError::InvalidEnvEncoding { name: key, .. } if key == name));
        super::error_tests::assert_redacted_chain(&error);
        assert!(matches!(
            super::error_tests::original::<VarError>(&error)?,
            VarError::NotUnicode(_)
        ));
    }
    Ok(())
}

#[test]
fn should_validate_full_job_config_without_building_adapters() -> TestResult {
    for name in [
        "VERTEX_AI_PROJECT_ID",
        "VERTEX_AI_LOCATION",
        "VERTEX_AI_MODEL",
    ] {
        let mut values = inputs();
        values.remove(name);
        assert!(
            matches!(parse(&values), Err(WiringError::MissingEnv { name: key }) if key == name)
        );
    }
    let mut values = inputs();
    values.insert("PERIODIC_MATCH_MAX_RUN_SECONDS", "7201".into());
    assert!(matches!(parse(&values), Err(WiringError::InvalidPolicy)));
    values.insert("PERIODIC_MATCH_MAX_RUN_SECONDS", "0".into());
    assert!(matches!(parse(&values), Err(WiringError::InvalidPolicy)));
    values.insert("PERIODIC_MATCH_MAX_RUN_SECONDS", "7200".into());
    assert!(parse(&values).is_ok());
    for name in [
        "PERIODIC_MATCH_PROJECTION_LAG_SECONDS",
        "PERIODIC_MATCH_REPLAY_OVERLAP_SECONDS",
    ] {
        let mut values = inputs();
        values.insert(name, u64::MAX.to_string().into());
        let error = parse(&values).err().ok_or("duration overflow accepted")?;
        assert!(matches!(error, WiringError::InvalidNumber { name: key, .. } if key == name));
        super::error_tests::assert_redacted_chain(&error);
        super::error_tests::original::<std::num::TryFromIntError>(&error)?;
    }
    Ok(())
}

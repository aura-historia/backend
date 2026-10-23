use super::*;

#[test]
fn should_register_the_complete_production_catalogue() {
    let mut command = std::process::Command::new(std::env::current_exe().unwrap());
    command
        .args([
            "--exact",
            "runtime::tests::production_registry_probe",
            "--ignored",
            "--nocapture",
        ])
        .env_clear()
        .env("OPENSEARCH_ENDPOINT_URL", "http://127.0.0.1:9")
        .env("STAGE", "ephemeral");
    if let Some(profile) = std::env::var_os("LLVM_PROFILE_FILE") {
        command.env("LLVM_PROFILE_FILE", profile);
    }
    let output = command.output().unwrap();
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("test result: ok. 1 passed"));
}

#[tokio::test]
#[ignore = "run in an isolated process by the production registry test"]
async fn production_registry_probe() {
    let options = sqlx::postgres::PgConnectOptions::new_without_pgpass()
        .host("127.0.0.1")
        .port(9)
        .username("fixture")
        .database("fixture");
    let pool = sqlx::postgres::PgPoolOptions::new().connect_lazy_with(options);
    let runtime = Runtime::from_pool(pool);
    let config = ProductListingConfig::from_getter(|_| None).unwrap();
    let registry = production_registry(
        &runtime,
        ProductListingDependencies::from_env(&config).unwrap(),
    )
    .unwrap();
    let manifest: serde_json::Value =
        serde_json::from_str(include_str!("../../authenticated-operations.json")).unwrap();
    for selected in manifest["operations"].as_array().unwrap() {
        let name = selected["operationId"].as_str().unwrap();
        assert_eq!(
            crate::AccessClass::Protected,
            registry.get(name).unwrap().policy().access
        );
    }
    let (tools, next) = registry.page(None).unwrap();
    assert_eq!(38, tools.len());
    assert!(next.is_none());
    for tool in tools {
        for (kind, schema) in [
            ("input", serde_json::to_value(tool.input_schema).unwrap()),
            ("output", serde_json::to_value(tool.output_schema).unwrap()),
        ] {
            assert_unsigned_bounds(&schema, &format!("{}.{kind}", tool.name));
        }
    }
}

fn assert_unsigned_bounds(value: &serde_json::Value, path: &str) {
    match value {
        serde_json::Value::Object(object) => {
            let bound = match object.get("format").and_then(serde_json::Value::as_str) {
                Some("uint8") => Some(u64::from(u8::MAX)),
                Some("uint16") => Some(u64::from(u16::MAX)),
                Some("uint32") => Some(u64::from(u32::MAX)),
                Some("uint64") => Some(u64::MAX),
                Some("uint") => Some(usize::MAX as u64),
                _ => None,
            };
            if let Some(bound) = bound {
                let maximum = object.get("maximum").and_then(serde_json::Value::as_u64);
                assert!(
                    maximum.is_some_and(|maximum| maximum <= bound),
                    "{path}: missing or inexact unsigned maximum: {value}"
                );
            }
            for (key, child) in object {
                assert_unsigned_bounds(child, &format!("{path}/{key}"));
            }
        }
        serde_json::Value::Array(items) => {
            for (index, child) in items.iter().enumerate() {
                assert_unsigned_bounds(child, &format!("{path}/{index}"));
            }
        }
        _ => {}
    }
}

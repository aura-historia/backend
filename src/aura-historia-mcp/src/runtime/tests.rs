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
        .env("STAGE", "ephemeral")
        .env(
            "PRODUCT_LISTING_INGESTION_QUEUE_URL",
            "http://127.0.0.1:9/fixture.fifo",
        )
        .env("AWS_REGION", "eu-central-1")
        .env("AWS_ACCESS_KEY_ID", "fixture")
        .env("AWS_SECRET_ACCESS_KEY", "fixture")
        .env("AWS_EC2_METADATA_DISABLED", "true");
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
    let registry =
        production_registry(&runtime, McpDependencies::from_env(&config).await.unwrap()).unwrap();
    let mut expected = std::collections::BTreeMap::new();
    for manifest in [
        include_str!("../../public-operations.json"),
        include_str!("../../authenticated-operations.json"),
        include_str!("../../partner-operations.json"),
    ] {
        let manifest: serde_json::Value = serde_json::from_str(manifest).unwrap();
        for entry in manifest["operations"].as_array().unwrap() {
            let name = entry["operationId"].as_str().unwrap().to_owned();
            let mut entry = entry.clone();
            for key in ["access", "requiredScopes", "annotations"] {
                if entry.get(key).is_none() {
                    entry[key] = manifest[key].clone();
                }
            }
            assert!(expected.insert(name, entry).is_none());
        }
    }
    let (mut tools, next) = registry.page(None).unwrap();
    assert_eq!(50, tools.len());
    let (rest, end) = registry.page(next.as_deref()).unwrap();
    assert_eq!(24, rest.len());
    assert!(end.is_none());
    tools.extend(rest);
    assert_eq!(74, expected.len());
    assert!(
        registry
            .get("putWoocommerceListingSourceIngestionConfiguration")
            .is_none()
    );
    assert_eq!(
        expected.keys().cloned().collect::<Vec<_>>(),
        tools
            .iter()
            .map(|tool| tool.name.to_string())
            .collect::<Vec<_>>()
    );
    for tool in tools {
        let entry = &expected[tool.name.as_ref()];
        let meta = serde_json::to_value(&tool.meta).unwrap();
        assert_eq!(entry["operationId"], meta["aura.historia/operationId"]);
        assert_eq!(entry["access"], meta["aura.historia/access"]);
        assert_eq!(
            entry["requiredScopes"],
            meta["aura.historia/requiredScopes"]
        );
        assert_eq!(
            entry["annotations"],
            serde_json::to_value(&tool.annotations).unwrap()
        );
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

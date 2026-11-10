use super::*;
use aura_historia_mcp::LambdaMcpRouter;
use aura_historia_runtime::{ProductListingConfig, Runtime};
use std::process::Command;

type TestError = application::error::BoxError;

#[test]
fn should_serve_the_complete_catalogue_from_one_warm_lambda_composition() -> Result<(), TestError> {
    let mut command = Command::new(std::env::current_exe()?);
    command
        .args([
            "--exact",
            "lambda_composition::lambda_composition_probe",
            "--ignored",
            "--nocapture",
        ])
        .env_clear()
        .envs([
            ("POSTGRES_HOST", "127.0.0.1"),
            ("POSTGRES_PORT", "9"),
            ("POSTGRES_DATABASE", "fixture"),
            ("POSTGRES_USERNAME", "fixture"),
            ("POSTGRES_PASSWORD", "fixture-password"),
            ("POSTGRES_TLS_ROOT_CERT", "/fixture/ca.pem"),
            ("STAGE", "ephemeral"),
            ("AWS_REGION", "eu-central-1"),
            ("AWS_ACCESS_KEY_ID", "fixture"),
            ("AWS_SECRET_ACCESS_KEY", "fixture"),
            ("AWS_EC2_METADATA_DISABLED", "true"),
            ("OPENSEARCH_ENDPOINT_URL", "http://127.0.0.1:9"),
        ]);
    if let Some(profile) = std::env::var_os("LLVM_PROFILE_FILE") {
        command.env("LLVM_PROFILE_FILE", profile);
    }
    let output = command.output()?;
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("test result: ok. 1 passed"));
    Ok(())
}

#[tokio::test]
#[ignore = "run by the clean-environment Lambda composition test"]
async fn lambda_composition_probe() -> Result<(), TestError> {
    let app = LambdaMcpRouter::new(
        Runtime::from_env().await?,
        TransportConfig {
            allowed_hosts: vec!["mcp.example.test".into()],
            ..Default::default()
        },
        ProductListingConfig::from_env()?,
    );
    let (first, concurrent) = tokio::join!(app.current(), app.current());
    let (first, concurrent) = (first?, concurrent?);
    let (_, listed, _) = post(first.clone(), "tools/list", json!({})).await;
    let mut expected = std::collections::BTreeSet::new();
    for manifest in [
        include_str!("../../public-operations.json"),
        include_str!("../../authenticated-operations.json"),
    ] {
        let manifest: Value = serde_json::from_str(manifest)?;
        for operation in manifest["operations"].as_array().unwrap() {
            assert_eq!(operation["name"], operation["operationId"]);
            assert!(expected.insert(operation["name"].as_str().unwrap().to_owned()));
        }
    }
    assert!(listed["result"].get("nextCursor").is_none());
    let tools = listed["result"]["tools"].as_array().unwrap();
    assert_eq!(37, tools.len());
    let actual = tools
        .iter()
        .map(|tool| tool["name"].as_str().unwrap().to_owned())
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(tools.len(), actual.len());
    assert_eq!(expected, actual);
    assert_eq!(listed, lambda_catalogue(first, json!({})).await?);
    assert_eq!(listed, post(concurrent, "tools/list", json!({})).await.1);
    assert_eq!(
        listed,
        post(app.current().await?, "tools/list", json!({})).await.1
    );
    Ok(())
}

async fn lambda_catalogue(app: Router, params: Value) -> Result<Value, TestError> {
    use lambda_http::{
        lambda_runtime::{Context, LambdaEvent},
        request::LambdaRequest,
    };
    let payload: LambdaRequest = serde_json::from_value(json!({
        "version":"2.0", "routeKey":"ANY /v1", "rawPath":"/v1", "rawQueryString":"",
        "headers":{
            "host":"mcp.example.test", "content-type":"application/json",
            "accept":"application/json,text/event-stream", "mcp-protocol-version":"2026-07-28",
            "mcp-method":"tools/list"
        },
        "requestContext":{
            "stage":"$default",
            "http":{"method":"POST","path":"/v1","protocol":"HTTP/1.1","sourceIp":"127.0.0.1","userAgent":"catalogue-fixture"}
        },
        "body":body("tools/list",params).to_string(), "isBase64Encoded":false
    }))?;
    let service = lambda_http::service_fn(move |request| {
        aura_historia_mcp::lambda::handle_http_api_v2_request(app.clone(), request)
    });
    let mut context = Context::default();
    context.deadline = (SystemTime::now().duration_since(UNIX_EPOCH)? + Duration::from_secs(30))
        .as_millis() as u64;
    let response = lambda_http::Adapter::from(service)
        .oneshot(LambdaEvent { payload, context })
        .await?;
    let response = serde_json::to_value(response)?;
    assert_eq!(200, response["statusCode"]);
    Ok(serde_json::from_str(response["body"].as_str().unwrap())?)
}

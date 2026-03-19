use crate::IntegrationTestService;
use crate::localstack::get_aws_config;
use async_trait::async_trait;
use aws_sdk_cloudformation::types::StackStatus;
use aws_tests_common::{CloudFormationOutput, set_cfn_output};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;
use tokio::sync::OnceCell;
use tracing::{debug, error, info};

const ARTIFACT_BUCKET: &str = "lambda-artifacts";
const STACK_NAME: &str = "acceptance-test-stack";
const STAGE_NAME: &str = "acceptance";
const COMMIT_SHA: &str = "local";

/// All Lambda binary names that the ephemeral CloudFormation stack requires.
///
/// Each entry corresponds to a Cargo binary target that produces a Lambda handler.
/// The product-pipeline-asg-scale-control Lambda is excluded because it is not
/// exercised by acceptance tests and its ASG/EC2 resources are removed from the
/// LocalStack-specific template.
const LAMBDA_BINARIES: &[&str] = &[
    "cognito-post-confirmation",
    "product-api",
    "product-watchlist-api",
    "notification-api",
    "user-api",
    "shop-api",
    "product-classification-api",
    "search-filter-api",
    "notification-send",
    "product-lambda-materialize-dynamodb",
    "product-lambda-materialize-opensearch",
    "shop-lambda-opensearch-index",
    "search-filter-lambda-opensearch-sync",
    "product-lambda-update-notify-user",
    "search-filter-lambda-percolate-product",
];

static CFN_CLIENT: OnceCell<aws_sdk_cloudformation::Client> = OnceCell::const_new();
async fn get_cfn_client() -> &'static aws_sdk_cloudformation::Client {
    CFN_CLIENT
        .get_or_init(|| async { aws_sdk_cloudformation::Client::new(get_aws_config().await) })
        .await
}

static S3_CLIENT: OnceCell<aws_sdk_s3::Client> = OnceCell::const_new();
async fn get_s3_client() -> &'static aws_sdk_s3::Client {
    S3_CLIENT
        .get_or_init(|| async { aws_sdk_s3::Client::new(get_aws_config().await) })
        .await
}

/// Service type representing a full CloudFormation stack deployment on LocalStack Pro.
///
/// When used with the `#[localstack_test]` macro, this service:
/// 1. Builds all Lambda binaries from the workspace
/// 2. Packages each binary into a ZIP (containing a `bootstrap` executable)
/// 3. Creates an S3 bucket and uploads all Lambda ZIPs
/// 4. Deploys the LocalStack-specific ephemeral CloudFormation template
/// 5. Waits for the stack to reach `CREATE_COMPLETE`
/// 6. Extracts stack outputs into [`CloudFormationOutput`] via `CFN_OUTPUT` env var
pub struct Cloudformation();

#[async_trait]
impl IntegrationTestService for Cloudformation {
    fn service_names(&self) -> &'static [&'static str] {
        &[
            "cloudformation",
            "lambda",
            "iam",
            "events",
            "pipes",
            "sqs",
            "cognito-idp",
            "dynamodb",
            "opensearch",
            "apigatewayv2",
            "s3",
            "ses",
        ]
    }

    async fn set_up(&self) {
        build_lambdas();
        create_artifact_bucket().await;
        package_and_upload_lambdas().await;
        deploy_stack().await;
        extract_and_set_cfn_outputs().await;
    }
}

/// Builds all Lambda function binaries using `cargo build --workspace`.
fn build_lambdas() {
    info!("Building Lambda binaries...");
    let workspace_dir = env!("CARGO_WORKSPACE_DIR");

    let status = Command::new("cargo")
        .args(["build", "--workspace"])
        .current_dir(workspace_dir)
        .status()
        .expect("shouldn't fail spawning cargo build");

    assert!(status.success(), "cargo build --workspace failed");
    info!("Lambda binaries built successfully.");
}

/// Creates the S3 artifact bucket in LocalStack.
async fn create_artifact_bucket() {
    let s3 = get_s3_client().await;
    s3.create_bucket()
        .bucket(ARTIFACT_BUCKET)
        .send()
        .await
        .expect("shouldn't fail creating artifact S3 bucket");
    debug!("Created S3 artifact bucket '{ARTIFACT_BUCKET}'.");
}

/// Packages each Lambda binary into a ZIP and uploads it to S3.
///
/// The ZIP contains a single file named `bootstrap` (required by the `provided.al2023` runtime).
/// The S3 key follows the pattern: `{binary_name}-{STAGE_NAME}-{COMMIT_SHA}.zip`
async fn package_and_upload_lambdas() {
    let workspace_dir = PathBuf::from(env!("CARGO_WORKSPACE_DIR"));
    let target_dir = workspace_dir.join("target").join("debug");

    for binary_name in LAMBDA_BINARIES {
        let binary_path = target_dir.join(binary_name);
        assert!(
            binary_path.exists(),
            "Lambda binary not found at '{}'. Ensure `cargo build --workspace` succeeded.",
            binary_path.display()
        );

        let zip_bytes = create_lambda_zip(&binary_path);
        let s3_key = format!("{binary_name}-{STAGE_NAME}-{COMMIT_SHA}.zip");

        get_s3_client()
            .await
            .put_object()
            .bucket(ARTIFACT_BUCKET)
            .key(&s3_key)
            .body(zip_bytes.into())
            .send()
            .await
            .unwrap_or_else(|e| panic!("shouldn't fail uploading '{s3_key}' to S3: {e}"));

        debug!("Uploaded Lambda ZIP '{s3_key}' to S3.");
    }

    info!(
        "All {} Lambda ZIPs uploaded to S3.",
        LAMBDA_BINARIES.len()
    );
}

/// Creates a ZIP archive containing the given binary renamed to `bootstrap`.
fn create_lambda_zip(binary_path: &Path) -> Vec<u8> {
    let binary_data =
        std::fs::read(binary_path).expect("shouldn't fail reading Lambda binary file");

    let mut buf = Vec::new();
    {
        let mut zip = zip::ZipWriter::new(std::io::Cursor::new(&mut buf));
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated)
            .unix_permissions(0o755);
        zip.start_file("bootstrap", options)
            .expect("shouldn't fail starting ZIP entry");
        zip.write_all(&binary_data)
            .expect("shouldn't fail writing binary to ZIP");
        zip.finish()
            .expect("shouldn't fail finishing ZIP archive");
    }
    buf
}

static CFN_TEMPLATE: &str = include_str!(concat!(
    env!("CARGO_WORKSPACE_DIR"),
    "cfn/ephemeral-localstack.yaml"
));

/// Deploys the CloudFormation stack on LocalStack and waits for completion.
async fn deploy_stack() {
    info!("Deploying CloudFormation stack '{STACK_NAME}'...");
    let cfn = get_cfn_client().await;

    cfn.create_stack()
        .stack_name(STACK_NAME)
        .template_body(CFN_TEMPLATE)
        .parameters(
            aws_sdk_cloudformation::types::Parameter::builder()
                .parameter_key("Stage")
                .parameter_value(STAGE_NAME)
                .build(),
        )
        .parameters(
            aws_sdk_cloudformation::types::Parameter::builder()
                .parameter_key("StageName")
                .parameter_value(STAGE_NAME)
                .build(),
        )
        .parameters(
            aws_sdk_cloudformation::types::Parameter::builder()
                .parameter_key("ArtifactBucket")
                .parameter_value(ARTIFACT_BUCKET)
                .build(),
        )
        .parameters(
            aws_sdk_cloudformation::types::Parameter::builder()
                .parameter_key("CommitSHA")
                .parameter_value(COMMIT_SHA)
                .build(),
        )
        .capabilities(aws_sdk_cloudformation::types::Capability::CapabilityNamedIam)
        .send()
        .await
        .expect("shouldn't fail creating CloudFormation stack");

    wait_for_stack_complete().await;
    info!("CloudFormation stack '{STACK_NAME}' deployed successfully.");
}

/// Polls the stack status until it reaches `CREATE_COMPLETE` or fails.
async fn wait_for_stack_complete() {
    let cfn = get_cfn_client().await;
    let mut retries = 600;

    loop {
        let describe = cfn
            .describe_stacks()
            .stack_name(STACK_NAME)
            .send()
            .await
            .expect("shouldn't fail describing stack");

        let stack = describe
            .stacks()
            .first()
            .expect("shouldn't fail getting stack from describe response");

        let status = stack
            .stack_status()
            .expect("shouldn't fail getting stack status");

        match status {
            StackStatus::CreateComplete => {
                debug!("Stack reached CREATE_COMPLETE.");
                return;
            }
            StackStatus::CreateInProgress => {
                retries -= 1;
                if retries <= 0 {
                    panic!("Stack creation timed out after 600 retries");
                }
                debug!(remaining_retries = retries, "Stack still creating...");
                tokio::time::sleep(Duration::from_secs(1)).await;
            }
            StackStatus::CreateFailed | StackStatus::RollbackComplete | StackStatus::RollbackInProgress => {
                let reason = stack.stack_status_reason().unwrap_or("unknown");
                error!(status = ?status, reason = reason, "Stack creation failed.");

                // Log stack events for debugging
                if let Ok(events) = cfn
                    .describe_stack_events()
                    .stack_name(STACK_NAME)
                    .send()
                    .await
                {
                    for event in events.stack_events() {
                        if let Some(reason) = event.resource_status_reason() {
                            error!(
                                resource = event.logical_resource_id().unwrap_or("?"),
                                status = ?event.resource_status(),
                                reason = reason,
                                "Stack event"
                            );
                        }
                    }
                }
                panic!("CloudFormation stack creation failed: {status:?} - {reason}");
            }
            other => {
                panic!("Unexpected stack status: {other:?}");
            }
        }
    }
}

/// Extracts CloudFormation stack outputs and sets the `CFN_OUTPUT` environment variable.
async fn extract_and_set_cfn_outputs() {
    let cfn = get_cfn_client().await;

    let describe = cfn
        .describe_stacks()
        .stack_name(STACK_NAME)
        .send()
        .await
        .expect("shouldn't fail describing stack for outputs");

    let stack = describe
        .stacks()
        .first()
        .expect("shouldn't fail getting stack from describe response");

    let outputs = stack.outputs();
    let mut output_map = serde_json::Map::new();
    for output in outputs {
        if let (Some(key), Some(value)) = (output.output_key(), output.output_value()) {
            output_map.insert(
                key.to_string(),
                serde_json::Value::String(value.to_string()),
            );
        }
    }

    let cfn_output_json = serde_json::Value::Object(output_map).to_string();
    debug!(cfn_output = %cfn_output_json, "Extracted CloudFormation outputs.");

    let cfn_output: CloudFormationOutput =
        serde_json::from_str(&cfn_output_json).unwrap_or_else(|e| {
            panic!("shouldn't fail parsing CFN outputs to CloudFormationOutput: {e}\nJSON: {cfn_output_json}")
        });

    set_cfn_output(cfn_output);
    info!("CloudFormation outputs set successfully.");
}

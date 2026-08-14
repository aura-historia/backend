use crate::IntegrationTestService;
use crate::cognito::Cognito;
use crate::dynamodb::clear_table_data;
use crate::localstack::{get_aws_config, get_endpoint_url};
use crate::opensearch::clear_all_indices;
use crate::ses::{Ses, clear_sent_emails};
use crate::sqs::drain_queues;
use async_trait::async_trait;
use aws_sdk_cloudformation::{error::ProvideErrorMetadata, types::StackStatus};
use aws_sdk_s3::types::{BucketLocationConstraint, CreateBucketConfiguration};
use aws_tests_common::{CloudFormationOutput, get_cfn_output, set_cfn_output};
use futures::stream::{self, StreamExt};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;
use tokio::sync::OnceCell;
use tracing::{debug, error, info};

const ARTIFACT_BUCKET: &str = "aura-historia-binary-artifacts-eu-central-1";
const STACK_NAME: &str = "acceptance-test-stack";
const COMMIT_SHA: &str = "local";

/// CDK stage synthesized for LocalStack acceptance tests.
///
/// This value is used for physical resource suffixes, Lambda artifact keys,
/// mail-template prefixes, and the runtime `STAGE` environment variable. The
/// Lambda OpenSearch client branches on `"ephemeral"` to use LocalStack's
/// unsigned OpenSearch proxy without username/password credentials.
const STAGE: &str = "ephemeral";

/// All Lambda binary names that the ephemeral CloudFormation stack requires.
///
/// Each entry corresponds to a Cargo binary target that produces a Lambda handler.
const LAMBDA_BINARIES: &[&str] = &[
    "cognito-post-confirmation",
    "cloudwatch-log-retention-lambda",
    "product-api",
    "product-api-partner",
    "product-watchlist-api",
    "notification-api",
    "user-api",
    "oauth-api",
    "newsletter-api",
    "partner-shop-application-api",
    "partner-shop-application-lambda",
    "shop-api",
    "webhook-api",
    "search-filter-api",
    "notification-send",
    "product-lambda-materialize-opensearch",
    "product-lambda-ingest-partner-products",
    "product-lambda-delete-product",
    "shop-lambda-opensearch-index",
    "shopify-lambda",
    "user-lambda-index-opensearch",
    "user-lambda-tier-update",
    "search-filter-lambda-opensearch-sync",
    "product-lambda-update-notify-user",
    "search-filter-lambda-percolate-product",
    "stripe-lambda",
    "stripe-api",
];

/// Guards the one-time CloudFormation stack setup.
///
/// Because the `#[aura_integration_test]` macro calls `set_up` before every test,
/// this cell ensures the expensive build → upload → deploy sequence runs only
/// once per test-process, regardless of how many tests exist.
static SETUP_ONCE: OnceCell<()> = OnceCell::const_new();

static CFN_CLIENT: OnceCell<aws_sdk_cloudformation::Client> = OnceCell::const_new();
async fn get_cfn_client() -> &'static aws_sdk_cloudformation::Client {
    CFN_CLIENT
        .get_or_init(|| async { aws_sdk_cloudformation::Client::new(get_aws_config().await) })
        .await
}

static S3_CLIENT: OnceCell<aws_sdk_s3::Client> = OnceCell::const_new();
async fn get_s3_client() -> &'static aws_sdk_s3::Client {
    S3_CLIENT
        .get_or_init(|| async {
            let s3_config = aws_sdk_s3::config::Builder::from(get_aws_config().await)
                .force_path_style(true)
                .build();
            aws_sdk_s3::Client::from_conf(s3_config)
        })
        .await
}

/// Service type representing a full CloudFormation stack deployment on LocalStack Pro.
///
/// When used with the `#[aura_integration_test]` macro, this service:
/// 1. Builds all Lambda binaries from the workspace
/// 2. Packages each binary into a ZIP (containing a `bootstrap` executable)
/// 3. Creates an S3 bucket and uploads all Lambda ZIPs
/// 4. Synthesizes the LocalStack-specific CDK stack template
/// 5. Deploys the synthesized template through CloudFormation
/// 6. Waits for the stack to reach `CREATE_COMPLETE`
/// 7. Extracts stack outputs into [`CloudFormationOutput`] via `CFN_OUTPUT` env var
pub struct Cloudformation();

#[async_trait]
impl IntegrationTestService for Cloudformation {
    fn service_names(&self) -> &'static [&'static str] {
        &[
            "cloudformation",
            "cloudfront",
            "lambda",
            "iam",
            "logs",
            "events",
            "pipes",
            "sqs",
            "cognito-idp",
            "dynamodb",
            "opensearch",
            "apigatewayv2",
            "s3",
            "sesv2",
            "stepfunctions",
        ]
    }

    async fn set_up(&self) {
        // Run the full deploy only once; all subsequent per-test calls are no-ops.
        SETUP_ONCE
            .get_or_init(|| async {
                build_lambdas();
                create_artifact_bucket().await;
                package_and_upload_lambdas().await;
                crate::S3().set_up().await;
                Ses().set_up().await;
                deploy_stack().await;
                extract_and_set_cfn_outputs().await;
                crate::opensearch::set_up_after_cloudformation().await;
            })
            .await;
    }

    /// Resets all mutable state created by CloudFormation-deployed services so
    /// that every test starts from a clean slate.
    ///
    /// Delegates to the same helpers used by the individual service
    /// `IntegrationTestService` implementations:
    ///
    /// - **DynamoDB** – scans and batch-deletes every item in the main table.
    /// - **OpenSearch** – deletes all documents from every standard index.
    /// - **SQS** – drains all queues (and their DLQs) via receive-delete loop,
    ///   avoiding the 60 s cooldown imposed by `purge_queue`.
    /// - **Cognito** – deletes every user in the user pool.
    /// - **SES** – clears all sent emails from LocalStack's in-memory store.
    async fn tear_down(&self) {
        let cfn = get_cfn_output();

        // ── DynamoDB ─────────────────────────────────────────────────────────
        clear_table_data(&cfn.dynamodb_table_1_name)
            .await
            .expect("shouldn't fail clearing DynamoDB table data");
        debug!(
            "Cleared DynamoDB table '{}' for test isolation",
            cfn.dynamodb_table_1_name
        );

        // ── OpenSearch ───────────────────────────────────────────────────────
        clear_all_indices().await;

        // ── SQS ──────────────────────────────────────────────────────────────
        drain_queues(vec![
            cfn.notification_send_queue_url.clone(),
            cfn.notification_send_dead_letter_queue_url.clone(),
            cfn.product_materialize_opensearch_queue_url.clone(),
            cfn.product_materialize_opensearch_dead_letter_queue_url
                .clone(),
            cfn.product_delete_product_queue_url.clone(),
            cfn.product_delete_product_dead_letter_queue_url.clone(),
            cfn.product_partner_ingest_queue_url.clone(),
            cfn.product_partner_ingest_dead_letter_queue_url.clone(),
            cfn.shop_opensearch_index_queue_url.clone(),
            cfn.shop_opensearch_index_dead_letter_queue_url.clone(),
            cfn.search_filter_open_search_sync_queue_url.clone(),
            cfn.search_filter_open_search_sync_dead_letter_queue_url
                .clone(),
            cfn.product_update_notify_user_queue_url.clone(),
            cfn.product_update_notify_user_dead_letter_queue_url.clone(),
        ])
        .await;
        debug!("Drained all SQS queues for test isolation");

        // ── Cognito ───────────────────────────────────────────────────────────
        Cognito().tear_down().await;

        // ── SES ──────────────────────────────────────────────────────────────
        clear_sent_emails().await;
        debug!("Cleared all sent SES emails for test isolation");

        debug!("Cloudformation tear_down complete: all state reset for test isolation.");
    }
}

/// Builds all Lambda function binaries using `cargo lambda build --workspace`.
///
/// `cargo-lambda` uses `cargo-zigbuild` under the hood to cross-compile against
/// a glibc version compatible with the `provided.al2023` Lambda runtime
/// (Amazon Linux 2023, glibc 2.34). A plain `cargo build` on a modern host
/// (e.g. Ubuntu 24.04 with glibc 2.39) produces binaries that fail to start
/// inside the Lambda container with "GLIBC_2.38 not found".
///
/// # Prerequisite
///
/// Install with: `cargo install cargo-lambda`
fn build_lambdas() {
    info!("Building Lambda binaries with cargo-lambda...");
    let workspace_dir = env!("CARGO_WORKSPACE_DIR");

    let status = Command::new("cargo")
        .args([
            "lambda",
            "build",
            "--workspace",
            "--release",
            "--locked",
            "--exclude",
            "crawler",
            "--exclude",
            "acceptance-tests",
            "--exclude",
            "aws-tests",
            "--exclude",
            "aws-tests-common",
            "--exclude",
            "smoking-tests",
            "--exclude",
            "ci-determinator",
        ])
        .current_dir(workspace_dir)
        .status()
        .expect(
            "shouldn't fail spawning cargo lambda build; \
             is cargo-lambda installed? Run: cargo install cargo-lambda",
        );

    assert!(
        status.success(),
        "cargo lambda build --workspace --release failed"
    );
    info!("Lambda binaries built successfully.");
}

/// Creates the S3 artifact bucket in LocalStack.
async fn create_artifact_bucket() {
    let s3 = get_s3_client().await;
    match s3
        .create_bucket()
        .bucket(ARTIFACT_BUCKET)
        .create_bucket_configuration(
            CreateBucketConfiguration::builder()
                .location_constraint(BucketLocationConstraint::EuCentral1)
                .build(),
        )
        .send()
        .await
    {
        Ok(_) => debug!("Created S3 artifact bucket '{ARTIFACT_BUCKET}'."),
        Err(error) if is_bucket_already_owned_error(&error) => {
            debug!("S3 artifact bucket '{ARTIFACT_BUCKET}' already exists; reusing it.");
        }
        Err(error) => panic!("shouldn't fail creating artifact S3 bucket: {error:?}"),
    }
}

fn is_bucket_already_owned_error(error: &impl ProvideErrorMetadata) -> bool {
    matches!(
        error.code(),
        Some("BucketAlreadyOwnedByYou" | "BucketAlreadyExists")
    )
}

/// Maximum number of concurrent S3 uploads.
///
/// LocalStack's S3 runs on a single reactor thread, so too many simultaneous
/// large uploads can stall the connection pipeline. Limiting concurrency keeps
/// throughput stable while still being significantly faster than sequential.
const MAX_CONCURRENT_UPLOADS: usize = 3;

/// Packages each Lambda binary into a ZIP and uploads it to S3 with bounded concurrency.
///
/// The ZIP contains a single file named `bootstrap` (required by the `provided.al2023` runtime).
/// The S3 key follows the pattern: `{binary_name}-{STAGE}-{COMMIT_SHA}.zip`
///
/// ZIP creation is deferred into each async task (via `spawn_blocking`) so that only
/// `MAX_CONCURRENT_UPLOADS` binaries are read and compressed at any given time, avoiding
/// excessive memory pressure from loading all binaries simultaneously.
async fn package_and_upload_lambdas() {
    let workspace_dir = PathBuf::from(env!("CARGO_WORKSPACE_DIR"));
    // cargo-lambda places each binary at target/lambda/{name}/bootstrap,
    // already named "bootstrap" as required by the provided.al2023 runtime.
    let target_dir = workspace_dir.join("target").join("lambda");

    let tasks: Vec<_> = LAMBDA_BINARIES
        .iter()
        .map(|binary_name| {
            let binary_path = target_dir.join(binary_name).join("bootstrap");
            assert!(
                binary_path.exists(),
                "Lambda binary not found at '{}'. \
                 Ensure `cargo lambda build --workspace --release` succeeded.",
                binary_path.display()
            );
            let s3_key = format!("{binary_name}-{STAGE}-{COMMIT_SHA}.zip");
            (binary_path, s3_key)
        })
        .collect();

    stream::iter(tasks.into_iter().map(|(binary_path, s3_key)| async move {
        let zip_bytes = tokio::task::spawn_blocking(move || create_lambda_zip(&binary_path))
            .await
            .expect("shouldn't fail spawning blocking ZIP task");

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
    }))
    .buffer_unordered(MAX_CONCURRENT_UPLOADS)
    .collect::<Vec<()>>()
    .await;

    info!("All {} Lambda ZIPs uploaded to S3.", LAMBDA_BINARIES.len());
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
        zip.finish().expect("shouldn't fail finishing ZIP archive");
    }
    buf
}

/// Synthesizes the CDK template used by the LocalStack acceptance-test stack.
fn synthesize_ephemeral_template() -> String {
    info!("Synthesizing CDK ephemeral stack template...");

    let endpoint_url = get_endpoint_url();
    let local_stack_mapped_port = endpoint_url.rsplit(':').next().unwrap_or("4566").to_owned();

    let output = Command::new("npm")
        .args([
            "--prefix",
            "infra",
            "--silent",
            "run",
            "synth",
            "--",
            "application-ephemeral",
            "--context",
            "stage=ephemeral",
            "--context",
            "singleStack=true",
            "--context",
        ])
        .arg(format!(
            "localStackMappedPort={local_stack_mapped_port}"
        ))
        .current_dir(env!("CARGO_WORKSPACE_DIR"))
        .output()
        .expect(
            "shouldn't fail spawning CDK synth; install Node.js and run `npm --prefix infra install`",
        );

    if !output.status.success() {
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        panic!("CDK synth for ephemeral stack failed\nstdout:\n{stdout}\nstderr:\n{stderr}");
    }

    let template = String::from_utf8(output.stdout)
        .expect("CDK synth output should be valid UTF-8 CloudFormation YAML");
    assert!(
        !template.trim().is_empty(),
        "CDK synth returned an empty ephemeral template"
    );

    info!("CDK ephemeral stack template synthesized successfully.");
    template
}

/// Deploys the CloudFormation stack on LocalStack and waits for completion.
async fn deploy_stack() {
    info!("Deploying CloudFormation stack '{STACK_NAME}'...");

    let template = synthesize_ephemeral_template();
    let template_key = "cdk-template.yaml";
    get_s3_client()
        .await
        .put_object()
        .bucket(ARTIFACT_BUCKET)
        .key(template_key)
        .body(template.into_bytes().into())
        .send()
        .await
        .expect("shouldn't fail uploading synthesized CDK template to S3");
    debug!("Uploaded synthesized CDK template to S3.");

    let template_url = format!("{}/{ARTIFACT_BUCKET}/{template_key}", get_endpoint_url());

    delete_existing_stack_if_present().await;

    let cfn = get_cfn_client().await;

    cfn.create_stack()
        .stack_name(STACK_NAME)
        .template_url(&template_url)
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

/// Deletes an acceptance-test stack left behind by an earlier failed run.
async fn delete_existing_stack_if_present() {
    let cfn = get_cfn_client().await;
    match cfn.describe_stacks().stack_name(STACK_NAME).send().await {
        Ok(response) => {
            let status = response
                .stacks()
                .first()
                .and_then(|stack| stack.stack_status())
                .cloned();

            if matches!(status, Some(StackStatus::DeleteComplete)) {
                debug!("Previous CloudFormation stack '{STACK_NAME}' is already deleted.");
                return;
            }

            info!(
                status = ?status,
                "Deleting previous CloudFormation stack '{STACK_NAME}' before redeploying."
            );
            cfn.delete_stack()
                .stack_name(STACK_NAME)
                .send()
                .await
                .expect("shouldn't fail deleting previous CloudFormation stack");
            wait_for_stack_deleted().await;
        }
        Err(error) if is_stack_not_found_error(&error) => {
            debug!("No previous CloudFormation stack '{STACK_NAME}' found.");
        }
        Err(error) => panic!("shouldn't fail checking previous CloudFormation stack: {error:?}"),
    }
}

fn is_stack_not_found_error(error: &impl ProvideErrorMetadata) -> bool {
    matches!(error.code(), Some("ValidationError" | "404" | "NotFound"))
        || error.message().is_some_and(|message| {
            message.contains("does not exist") || message.contains("not found")
        })
}

async fn wait_for_stack_deleted() {
    let cfn = get_cfn_client().await;
    let mut retries = 600;

    loop {
        match cfn.describe_stacks().stack_name(STACK_NAME).send().await {
            Ok(response) => {
                let stack = response
                    .stacks()
                    .first()
                    .expect("shouldn't fail getting stack from describe response");
                let status = stack
                    .stack_status()
                    .expect("shouldn't fail getting stack status");

                match status {
                    StackStatus::DeleteComplete => {
                        debug!("Previous stack reached DELETE_COMPLETE.");
                        return;
                    }
                    StackStatus::DeleteInProgress => {
                        retries -= 1;
                        if retries <= 0 {
                            panic!("Stack deletion timed out after 600 retries");
                        }
                        debug!(
                            remaining_retries = retries,
                            "Previous stack still deleting..."
                        );
                        tokio::time::sleep(Duration::from_secs(1)).await;
                    }
                    StackStatus::DeleteFailed => {
                        let reason = stack.stack_status_reason().unwrap_or("unknown");
                        panic!("CloudFormation stack deletion failed: {status:?} - {reason}");
                    }
                    other => {
                        debug!(status = ?other, "Waiting for previous stack deletion to settle...");
                        tokio::time::sleep(Duration::from_secs(1)).await;
                    }
                }
            }
            Err(error) if is_stack_not_found_error(&error) => {
                debug!("Previous CloudFormation stack '{STACK_NAME}' no longer exists.");
                return;
            }
            Err(error) => panic!("shouldn't fail waiting for stack deletion: {error:?}"),
        }
    }
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
            StackStatus::CreateFailed
            | StackStatus::RollbackComplete
            | StackStatus::RollbackInProgress => {
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
            let value = if key == "ApiGatewayEndpointUrl" {
                localize_apigw_url(value)
            } else {
                value.to_string()
            };
            output_map.insert(key.to_string(), serde_json::Value::String(value));
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

/// Rewrites a LocalStack-generated API Gateway endpoint URL so it is reachable
/// from the test process running **outside** the Docker container.
///
/// LocalStack computes output URLs using two values that are only valid from
/// inside the container:
///
/// * The **`amazonaws.com` hostname** – not resolvable from the host machine.
/// * The **container-internal port `4566`** – not the host-mapped random port.
///
/// This function replaces both:
///
/// * Hostname → `{api-id}.execute-api.localhost.localstack.cloud`
///   (`*.localhost.localstack.cloud` is a public wildcard DNS record that
///   resolves to `127.0.0.1`, so no `/etc/hosts` changes are needed.)
/// * Port → the actual host-mapped port extracted from [`get_endpoint_url()`].
///
/// # Example
///
/// ```text
/// input:  "https://46f9640d.execute-api.amazonaws.com:4566/ephemeral"
/// output: "http://46f9640d.execute-api.localhost.localstack.cloud:54321/ephemeral"
/// ```
fn localize_apigw_url(cfn_url: &str) -> String {
    // get_endpoint_url() → "http://localhost:{mapped-port}"
    let mapped_port = get_endpoint_url().rsplit(':').next().unwrap_or("4566");

    // Strip scheme to get "{host}:{internal-port}/{stage}/..."
    let rest = cfn_url
        .trim_start_matches("https://")
        .trim_start_matches("http://");

    // Split host:port from the path
    let (host_part, path) = rest.split_once('/').unwrap_or((rest, ""));

    // Drop the port from the host label
    let host = host_part.split(':').next().unwrap_or(host_part);

    // The API ID is the first DNS label (e.g. "46f9640d")
    let api_id = host.split('.').next().unwrap_or(host);

    format!("http://{api_id}.execute-api.localhost.localstack.cloud:{mapped_port}/{path}")
}

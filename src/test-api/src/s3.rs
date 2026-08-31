use crate::IntegrationTestService;
use async_trait::async_trait;

/// Service type representing S3 mail-template bucket setup for LocalStack-based tests.
///
/// When the `cloudformation` feature is enabled, [`set_up`](IntegrationTestService::set_up):
/// 1. Creates the `aura-historia-mail-templates-eu-central-1` S3 bucket
/// 2. Compiles all MJML mail templates to HTML using [`mrml`]
/// 3. Uploads the compiled HTML files with S3 keys matching the paths
///    expected by [`MailTemplate::as_s3_blob_str`]
///
/// Without the `cloudformation` feature, this is a no-op that only starts
/// the S3 service in LocalStack.
pub struct S3();

#[async_trait]
impl IntegrationTestService for S3 {
    fn service_names(&self) -> &'static [&'static str] {
        &["s3"]
    }

    async fn set_up(&self) {
        #[cfg(feature = "cloudformation")]
        mail_templates::create_bucket_and_upload().await;
    }
}

#[cfg(feature = "cloudformation")]
mod mail_templates {
    use crate::localstack::get_aws_config;
    use aws_sdk_s3::{
        error::ProvideErrorMetadata,
        types::{BucketLocationConstraint, CreateBucketConfiguration},
    };
    use futures::stream::{self, StreamExt};
    use std::path::PathBuf;
    use tokio::sync::OnceCell;
    use tracing::{debug, info};

    /// The S3 bucket name used by Lambdas to fetch compiled mail templates.
    ///
    /// Must match the `S3_BUCKET_NAME_TEMPLATES` value synthesized by the
    /// CDK ephemeral stack.
    const MAIL_TEMPLATE_BUCKET: &str = "aura-historia-mail-templates-eu-central-1";

    /// Stage injected into the S3 key prefix.
    ///
    /// Mirrors `STAGE` in [`crate::cloudformation`].
    const STAGE: &str = "ephemeral";

    /// Commit SHA injected into the S3 key prefix.
    ///
    /// Mirrors `COMMIT_SHA` in [`crate::cloudformation`].
    const COMMIT_SHA: &str = "local";

    /// Maximum number of concurrent S3 uploads.
    const MAX_CONCURRENT_UPLOADS: usize = 3;

    /// All MJML mail template source files, relative to the workspace root.
    ///
    /// Each entry corresponds to one combination of
    /// [`MailTemplateType::as_s3_dir_str`] × [`LanguageData::as_str`].
    const MJML_TEMPLATES: &[&str] = &[
        "mjml/watchlist/product-update/price/de.mjml",
        "mjml/watchlist/product-update/price/en.mjml",
        "mjml/watchlist/product-update/price/es.mjml",
        "mjml/watchlist/product-update/price/fr.mjml",
        "mjml/watchlist/product-update/price/it.mjml",
        "mjml/watchlist/product-update/availability/de.mjml",
        "mjml/watchlist/product-update/availability/en.mjml",
        "mjml/watchlist/product-update/availability/es.mjml",
        "mjml/watchlist/product-update/availability/fr.mjml",
        "mjml/watchlist/product-update/availability/it.mjml",
        "mjml/search-filter/match/de.mjml",
        "mjml/search-filter/match/en.mjml",
        "mjml/search-filter/match/es.mjml",
        "mjml/search-filter/match/fr.mjml",
        "mjml/search-filter/match/it.mjml",
        "mjml/partnership-application/approval/de.mjml",
        "mjml/partnership-application/approval/en.mjml",
        "mjml/partnership-application/approval/es.mjml",
        "mjml/partnership-application/approval/fr.mjml",
        "mjml/partnership-application/approval/it.mjml",
        "mjml/partnership-application/rejection/de.mjml",
        "mjml/partnership-application/rejection/en.mjml",
        "mjml/partnership-application/rejection/es.mjml",
        "mjml/partnership-application/rejection/fr.mjml",
        "mjml/partnership-application/rejection/it.mjml",
    ];

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

    /// Creates the mail-template S3 bucket, compiles every MJML template to
    /// HTML with [`mrml`], and uploads the result.
    ///
    /// The S3 key for each template is
    /// `{STAGE}/{COMMIT_SHA}/{dir}/{lang}.html`
    /// where `{dir}/{lang}` equals [`MailTemplate::as_s3_blob_str`], e.g.
    /// `ephemeral/local/mjml/watchlist/product-update/price/en.html`.
    pub(super) async fn create_bucket_and_upload() {
        create_mail_template_bucket().await;
        compile_and_upload_templates().await;
    }

    async fn create_mail_template_bucket() {
        let s3 = get_s3_client().await;
        match s3
            .create_bucket()
            .bucket(MAIL_TEMPLATE_BUCKET)
            .create_bucket_configuration(
                CreateBucketConfiguration::builder()
                    .location_constraint(BucketLocationConstraint::EuCentral1)
                    .build(),
            )
            .send()
            .await
        {
            Ok(_) => debug!("Created S3 mail template bucket '{MAIL_TEMPLATE_BUCKET}'."),
            Err(error) if is_bucket_already_owned_error(&error) => {
                debug!("S3 mail template bucket '{MAIL_TEMPLATE_BUCKET}' already exists.");
            }
            Err(error) => panic!("shouldn't fail creating mail template S3 bucket: {error}"),
        };
    }

    fn is_bucket_already_owned_error(error: &impl ProvideErrorMetadata) -> bool {
        matches!(
            error.code(),
            Some("BucketAlreadyOwnedByYou" | "BucketAlreadyExists")
        )
    }

    async fn compile_and_upload_templates() {
        let workspace_dir = PathBuf::from(env!("CARGO_WORKSPACE_DIR"));

        let tasks: Vec<_> = MJML_TEMPLATES
            .iter()
            .map(|template_rel_path| {
                let mjml_path = workspace_dir.join(template_rel_path);
                assert!(
                    mjml_path.exists(),
                    "MJML template not found at '{}'. \
                     Ensure the mjml/ directory exists in the workspace root.",
                    mjml_path.display()
                );

                // Derive S3 key: {STAGE}/{COMMIT_SHA}/{dir}/{lang}.html
                // e.g. "ephemeral/local/mjml/watchlist/product-update/price/de.html"
                let without_ext = template_rel_path
                    .strip_suffix(".mjml")
                    .expect("template path should end with .mjml");
                let s3_key = format!("{STAGE}/{COMMIT_SHA}/{without_ext}.html");

                (mjml_path, s3_key)
            })
            .collect();

        stream::iter(tasks.into_iter().map(|(mjml_path, s3_key)| async move {
            // Read the MJML source
            let mjml_content = tokio::fs::read_to_string(&mjml_path)
                .await
                .unwrap_or_else(|e| {
                    panic!(
                        "shouldn't fail reading MJML template '{}': {e}",
                        mjml_path.display()
                    )
                });

            // Compile MJML → HTML using mrml (blocking, offloaded to a thread)
            let html: String = tokio::task::spawn_blocking(move || {
                let root = mrml::parse(&mjml_content).unwrap_or_else(|e| {
                    panic!("shouldn't fail parsing MJML template: {e}");
                });
                root.element
                    .render(&mrml::prelude::render::RenderOptions::default())
                    .unwrap_or_else(|e| {
                        panic!("shouldn't fail rendering MJML template to HTML: {e}");
                    })
            })
            .await
            .expect("shouldn't fail spawning blocking MJML compile task");

            // Upload the compiled HTML to S3
            get_s3_client()
                .await
                .put_object()
                .bucket(MAIL_TEMPLATE_BUCKET)
                .key(&s3_key)
                .body(html.into_bytes().into())
                .send()
                .await
                .unwrap_or_else(|e| {
                    panic!("shouldn't fail uploading mail template '{s3_key}' to S3: {e}")
                });
            debug!("Uploaded compiled mail template '{s3_key}' to S3.");
        }))
        .buffer_unordered(MAX_CONCURRENT_UPLOADS)
        .collect::<Vec<()>>()
        .await;

        info!(
            "All {} mail templates compiled and uploaded to S3 bucket '{MAIL_TEMPLATE_BUCKET}'.",
            MJML_TEMPLATES.len()
        );
    }
}

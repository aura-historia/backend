use super::*;
use crate::{Isolation, TestContext};
use aws_sdk_account::error::ProvideErrorMetadata;
use aws_sdk_s3::types::{BucketLocationConstraint, CreateBucketConfiguration};
use aws_sdk_sesv2::types::{Body, Content, Destination, EmailContent, Message};

fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
}

async fn s3_client() -> aws_sdk_s3::Client {
    aws_sdk_s3::Client::from_conf(
        aws_sdk_s3::config::Builder::from(get_aws_config().await)
            .force_path_style(true)
            .build(),
    )
}

struct AwsResources {
    queue: String,
    bucket: String,
    sender: String,
    configuration_set: String,
}

async fn create_resources(context: &TestContext) -> AwsResources {
    let queue = context.queue_name("runtime", false).await.unwrap();
    let bucket = context.bucket_name("runtime").await.unwrap();
    let sender = context.register_sender("runtime").await.unwrap();
    let configuration_set = context.configuration_set_name("runtime").await.unwrap();
    let sqs = crate::get_sqs_client().await;
    let queue_url = sqs
        .create_queue()
        .queue_name(&queue)
        .send()
        .await
        .unwrap()
        .queue_url()
        .unwrap()
        .to_owned();
    sqs.send_message()
        .queue_url(queue_url)
        .message_body("runtime marker")
        .send()
        .await
        .unwrap();
    write_s3_marker(&bucket).await;
    send_ses_marker(&sender, &configuration_set).await;
    AwsResources {
        queue,
        bucket,
        sender,
        configuration_set,
    }
}

async fn write_s3_marker(bucket: &str) {
    let s3 = s3_client().await;
    s3.create_bucket()
        .bucket(bucket)
        .create_bucket_configuration(
            CreateBucketConfiguration::builder()
                .location_constraint(BucketLocationConstraint::EuCentral1)
                .build(),
        )
        .send()
        .await
        .unwrap();
    s3.put_object()
        .bucket(bucket)
        .key("marker")
        .body("runtime marker".as_bytes().to_vec().into())
        .send()
        .await
        .unwrap();
}

async fn send_ses_marker(sender: &str, configuration_set: &str) {
    crate::verify_sender_identity(sender).await;
    let ses = crate::get_ses_client().await;
    ses.create_configuration_set()
        .configuration_set_name(configuration_set)
        .send()
        .await
        .unwrap();
    ses.send_email()
        .from_email_address(sender)
        .configuration_set_name(configuration_set)
        .destination(
            Destination::builder()
                .to_addresses("runtime-recipient@example.test")
                .build(),
        )
        .content(
            EmailContent::builder()
                .simple(
                    Message::builder()
                        .subject(Content::builder().data("Runtime marker").build().unwrap())
                        .body(
                            Body::builder()
                                .text(Content::builder().data("runtime marker").build().unwrap())
                                .build(),
                        )
                        .build(),
                )
                .build(),
        )
        .send()
        .await
        .unwrap();
}

async fn read_resources(resources: &AwsResources) {
    let AwsResources {
        queue,
        bucket,
        sender,
        ..
    } = resources;
    let sqs = crate::get_sqs_client().await;
    let queue_url = sqs
        .get_queue_url()
        .queue_name(queue)
        .send()
        .await
        .unwrap()
        .queue_url()
        .unwrap()
        .to_owned();
    let received = sqs
        .receive_message()
        .queue_url(queue_url)
        .send()
        .await
        .unwrap();
    assert_eq!(Some("runtime marker"), received.messages()[0].body());
    let object = s3_client()
        .await
        .get_object()
        .bucket(bucket)
        .key("marker")
        .send()
        .await
        .unwrap();
    assert_eq!(
        b"runtime marker",
        object.body.collect().await.unwrap().into_bytes().as_ref()
    );
    assert!(
        crate::get_ses_client()
            .await
            .get_email_identity()
            .email_identity(sender)
            .send()
            .await
            .unwrap()
            .verified_for_sending_status()
    );
    let emails = crate::get_sent_emails_from(sender).await;
    assert_eq!(1, emails.len());
    assert_eq!("Runtime marker", emails[0].subject);
}

async fn assert_resources_removed(resources: AwsResources) {
    let AwsResources {
        queue,
        bucket,
        sender,
        configuration_set,
    } = resources;
    let queue_error = crate::get_sqs_client()
        .await
        .get_queue_url()
        .queue_name(queue)
        .send()
        .await
        .unwrap_err();
    assert!(
        queue_error
            .as_service_error()
            .unwrap()
            .is_queue_does_not_exist()
    );
    let bucket_error = s3_client()
        .await
        .head_bucket()
        .bucket(bucket)
        .send()
        .await
        .unwrap_err();
    assert!(bucket_error.as_service_error().unwrap().is_not_found());
    let ses = crate::get_ses_client().await;
    let identity_error = ses
        .get_email_identity()
        .email_identity(&sender)
        .send()
        .await
        .unwrap_err();
    assert!(
        identity_error
            .as_service_error()
            .unwrap()
            .is_not_found_exception()
    );
    let set_error = ses
        .get_configuration_set()
        .configuration_set_name(configuration_set)
        .send()
        .await
        .unwrap_err();
    assert_eq!(
        Some("NotFoundException"),
        set_error.as_service_error().unwrap().code()
    );
    assert!(crate::get_sent_emails_from(&sender).await.is_empty());
}

#[test]
fn should_own_aws_connections_across_setup_reads_and_cleanup_runtimes() {
    let setup = runtime();
    let (session, resources) = setup.block_on(async {
        get_aws_emulator(RUNTIME_TEST_SERVICES, &[]).await;
        assert!(
            get_aws_config().await.http_client().is_none(),
            "shared fixture configuration must not retain a runtime's HTTP connection pool"
        );
        let session = TestContext::acquire("AWS runtime lifetime", Isolation::Isolated)
            .await
            .unwrap();
        let context = session.context().clone();
        let resources = context.scope(create_resources(&context)).await;
        (session, resources)
    });
    drop(setup);

    let reads = runtime();
    reads.block_on(read_resources(&resources));
    drop(reads);

    let cleanup = runtime();
    cleanup.block_on(async {
        session.finish().await.unwrap();
        assert_resources_removed(resources).await;
    });
}

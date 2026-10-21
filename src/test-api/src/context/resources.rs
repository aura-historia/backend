use super::{Kind, Manifest, Resource, Result, failure};
#[cfg(any(feature = "sqs", feature = "s3", feature = "ses"))]
use aws_sdk_account::error::ProvideErrorMetadata;

pub(super) async fn clean(manifest: &Manifest) -> Result<()> {
    for resource in manifest.resources.iter().rev() {
        clean_resource(resource).await?;
    }
    Ok(())
}

async fn clean_resource(resource: &Resource) -> Result<()> {
    match resource.kind {
        Kind::Database => {
            #[cfg(feature = "postgres")]
            super::postgres::remove(&resource.name).await?;
            #[cfg(not(feature = "postgres"))]
            return Err(failure("slot recovery needs the postgres fixture feature"));
        }
        Kind::Index => {
            #[cfg(feature = "opensearch")]
            delete_index(&resource.name).await?;
            #[cfg(not(feature = "opensearch"))]
            return Err(failure(
                "slot recovery needs the opensearch fixture feature",
            ));
        }
        Kind::Queue => {
            #[cfg(feature = "sqs")]
            delete_queue(&resource.name).await?;
            #[cfg(not(feature = "sqs"))]
            return Err(failure("slot recovery needs the sqs fixture feature"));
        }
        Kind::Bucket => {
            #[cfg(feature = "s3")]
            delete_bucket(&resource.name).await?;
            #[cfg(not(feature = "s3"))]
            return Err(failure("slot recovery needs the s3 fixture feature"));
        }
        Kind::Sender => {
            #[cfg(feature = "ses")]
            delete_sender(&resource.name).await?;
            #[cfg(not(feature = "ses"))]
            return Err(failure("slot recovery needs the ses fixture feature"));
        }
        Kind::ConfigurationSet => {
            #[cfg(feature = "ses")]
            delete_configuration_set(&resource.name).await?;
            #[cfg(not(feature = "ses"))]
            return Err(failure("slot recovery needs the ses fixture feature"));
        }
    }
    Ok(())
}

#[cfg(any(feature = "sqs", feature = "s3", feature = "ses"))]
fn missing(error: &impl ProvideErrorMetadata) -> bool {
    matches!(
        error.code(),
        Some(
            "NoSuchBucket"
                | "NoSuchKey"
                | "NotFound"
                | "NotFoundException"
                | "QueueDoesNotExist"
                | "AWS.SimpleQueueService.NonExistentQueue"
        )
    )
}

#[cfg(feature = "opensearch")]
async fn delete_index(name: &str) -> Result<()> {
    let response = crate::get_opensearch_client()
        .await
        .indices()
        .delete(opensearch::indices::IndicesDeleteParts::Index(&[name]))
        .send()
        .await?;
    if response.status_code().as_u16() != 404 {
        response.error_for_status_code()?;
    }
    Ok(())
}

#[cfg(feature = "sqs")]
async fn delete_queue(name: &str) -> Result<()> {
    let url = format!("{}/000000000000/{name}", crate::aws::get_endpoint_url());
    if let Err(error) = crate::get_sqs_client()
        .await
        .delete_queue()
        .queue_url(url)
        .send()
        .await
        && !error.as_service_error().is_some_and(missing)
    {
        return Err(error.into());
    }
    Ok(())
}

#[cfg(feature = "s3")]
async fn delete_bucket(name: &str) -> Result<()> {
    let client = aws_sdk_s3::Client::from_conf(
        aws_sdk_s3::config::Builder::from(crate::aws::get_aws_config().await)
            .force_path_style(true)
            .build(),
    );
    loop {
        let objects = match client.list_objects_v2().bucket(name).send().await {
            Ok(objects) => objects,
            Err(error) if error.as_service_error().is_some_and(missing) => return Ok(()),
            Err(error) => return Err(error.into()),
        };
        if objects.contents().is_empty() {
            break;
        }
        for object in objects.contents() {
            let key = object
                .key()
                .ok_or_else(|| failure("owned S3 object has no key"))?;
            client.delete_object().bucket(name).key(key).send().await?;
        }
    }
    if let Err(error) = client.delete_bucket().bucket(name).send().await
        && !error.as_service_error().is_some_and(missing)
    {
        return Err(error.into());
    }
    Ok(())
}

#[cfg(feature = "ses")]
async fn delete_sender(source: &str) -> Result<()> {
    crate::ses::clear_source(source).await?;
    if let Err(error) = crate::get_ses_client()
        .await
        .delete_email_identity()
        .email_identity(source)
        .send()
        .await
        && !error.as_service_error().is_some_and(missing)
    {
        return Err(error.into());
    }
    Ok(())
}

#[cfg(feature = "ses")]
async fn delete_configuration_set(name: &str) -> Result<()> {
    if let Err(error) = crate::get_ses_client()
        .await
        .delete_configuration_set()
        .configuration_set_name(name)
        .send()
        .await
        && !error.as_service_error().is_some_and(missing)
    {
        return Err(error.into());
    }
    Ok(())
}

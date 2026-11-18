use google_cloud_auth::credentials::{AccessTokenCredentials, Builder};

const GOOGLE_CLOUD_PLATFORM_SCOPE: &str = "https://www.googleapis.com/auth/cloud-platform";
const GOOGLE_APPLICATION_CREDENTIALS_ENV: &str = "GOOGLE_APPLICATION_CREDENTIALS";

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum GoogleCredentialsError {
    #[error("GOOGLE_APPLICATION_CREDENTIALS must name a readable credentials file")]
    Missing,
    #[error("failed to configure Google application credentials")]
    Build,
}

/// Google application default credentials from the file `GOOGLE_APPLICATION_CREDENTIALS` names.
/// The process never falls back to a developer's local credentials or a metadata server, so a
/// missing file fails at startup instead of on the first provider call. The returned credentials
/// refresh their access token as needed.
pub fn google_application_default_credentials()
-> Result<AccessTokenCredentials, GoogleCredentialsError> {
    let path = std::env::var_os(GOOGLE_APPLICATION_CREDENTIALS_ENV)
        .filter(|path| !path.is_empty())
        .ok_or(GoogleCredentialsError::Missing)?;
    if !std::path::Path::new(&path).is_file() {
        return Err(GoogleCredentialsError::Missing);
    }
    Builder::default()
        .with_scopes([GOOGLE_CLOUD_PLATFORM_SCOPE])
        .build_access_token_credentials()
        .map_err(|_| GoogleCredentialsError::Build)
}

#[cfg(test)]
mod tests {
    use google_cloud_auth::credentials::external_account;

    /// The host's keyless Vertex AI configuration (docs/infra.md#vertex-ai-credentials): an AWS
    /// external account without metadata-server URLs, so the client signs with the host IAM
    /// user's keys from the environment instead of looking for an EC2 instance role.
    #[tokio::test]
    async fn should_accept_the_hosts_keyless_aws_external_account_configuration() {
        let configuration = serde_json::json!({
            "type": "external_account",
            "audience": "//iam.googleapis.com/projects/123456789012/locations/global/workloadIdentityPools/aura-historia/providers/aws",
            "subject_token_type": "urn:ietf:params:aws:token-type:aws4_request",
            "token_url": "https://sts.googleapis.com/v1/token",
            "credential_source": {
                "environment_id": "aws1",
                "regional_cred_verification_url": "https://sts.{region}.amazonaws.com?Action=GetCallerIdentity&Version=2011-06-15"
            }
        });

        assert!(
            external_account::Builder::new(configuration)
                .with_scopes([super::GOOGLE_CLOUD_PLATFORM_SCOPE])
                .build_access_token_credentials()
                .is_ok()
        );
    }
}

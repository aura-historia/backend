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

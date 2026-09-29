use google_cloud_auth::credentials::{AccessTokenCredentials, Builder};
use platform_google_adc::{MaterializationError, materialize_google_application_credentials};
use std::{
    env,
    ffi::OsStr,
    path::{Path, PathBuf},
};
use thiserror::Error;

const GOOGLE_CLOUD_PLATFORM_SCOPE: &str = "https://www.googleapis.com/auth/cloud-platform";
const GOOGLE_ADC_CREDENTIALS_JSON_ENV: &str = "AURA_HISTORIA_GOOGLE_ADC_CREDENTIALS_JSON";
const GOOGLE_APPLICATION_CREDENTIALS_ENV: &str = "GOOGLE_APPLICATION_CREDENTIALS";
const GOOGLE_ADC_CREDENTIALS_DIRECTORY: &str = "/tmp/aura-historia-google-adc";

#[derive(Debug, Error, PartialEq, Eq)]
pub enum GoogleAdcError {
    #[error("Google application credentials are unavailable")]
    Missing,
    #[error("invalid Google application credentials")]
    Invalid,
    #[error("failed to prepare private Google application credentials file")]
    File,
    #[error("failed to configure Google application credentials")]
    Build,
}

/// Call from synchronous `main`, before starting Tokio or any other worker threads.
pub fn materialize_google_application_credentials_from_env() -> Result<(), GoogleAdcError> {
    let credentials_path = prepare_credentials(
        env::var_os(GOOGLE_ADC_CREDENTIALS_JSON_ENV).as_deref(),
        Path::new(GOOGLE_ADC_CREDENTIALS_DIRECTORY),
    )?;
    // SAFETY: callers run this in synchronous process initialization before spawning threads.
    unsafe {
        env::set_var(GOOGLE_APPLICATION_CREDENTIALS_ENV, credentials_path);
        env::remove_var(GOOGLE_ADC_CREDENTIALS_JSON_ENV);
    }
    Ok(())
}

fn prepare_credentials(json: Option<&OsStr>, directory: &Path) -> Result<PathBuf, GoogleAdcError> {
    let json = json.ok_or(GoogleAdcError::Missing)?;
    let json = json.to_str().ok_or(GoogleAdcError::Invalid)?;
    materialize_google_application_credentials(json, directory).map_err(|error| match error {
        MaterializationError::Invalid => GoogleAdcError::Invalid,
        MaterializationError::File => GoogleAdcError::File,
    })
}

/// The returned credentials refresh their access token as needed, including after warm idle.
pub fn google_application_default_credentials() -> Result<AccessTokenCredentials, GoogleAdcError> {
    Builder::default()
        .with_scopes([GOOGLE_CLOUD_PLATFORM_SCOPE])
        .build_access_token_credentials()
        .map_err(|_| GoogleAdcError::Build)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn injected_json_materializes_at_the_adc_path() {
        let directory =
            env::temp_dir().join(format!("google-adc-lambda-valid-{}", std::process::id()));
        let json = r#"{"type":"service_account"}"#;
        let path = prepare_credentials(Some(OsStr::new(json)), &directory).unwrap();
        assert_eq!(path, directory.join("application_default_credentials.json"));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), json);
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn requires_injected_json_without_falling_back_to_local_adc() {
        let directory =
            env::temp_dir().join(format!("google-adc-lambda-missing-{}", std::process::id()));
        assert_eq!(
            prepare_credentials(None, &directory),
            Err(GoogleAdcError::Missing)
        );
        assert_eq!(
            prepare_credentials(Some(OsStr::new("")), &directory),
            Err(GoogleAdcError::Invalid)
        );
        for invalid in ["[]", "null", "not-json"] {
            assert_eq!(
                prepare_credentials(Some(OsStr::new(invalid)), &directory),
                Err(GoogleAdcError::Invalid)
            );
        }
        assert!(!directory.exists());
    }

    #[test]
    fn rejects_invalid_destination_without_leaking_credentials() {
        let directory =
            env::temp_dir().join(format!("google-adc-lambda-file-{}", std::process::id()));
        std::fs::write(&directory, "not a directory").unwrap();
        let error = prepare_credentials(Some(OsStr::new(r#"{"secret":"sensitive"}"#)), &directory)
            .unwrap_err();
        assert_eq!(error, GoogleAdcError::File);
        assert!(!format!("{error:?} {error}").contains("sensitive"));
        std::fs::remove_file(directory).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn rejects_non_unicode_injected_json() {
        use std::os::unix::ffi::OsStrExt;
        let directory = env::temp_dir().join(format!(
            "google-adc-lambda-nonunicode-{}",
            std::process::id()
        ));
        assert_eq!(
            prepare_credentials(Some(OsStr::from_bytes(b"\xff")), &directory),
            Err(GoogleAdcError::Invalid)
        );
        assert!(!directory.exists());
    }
}

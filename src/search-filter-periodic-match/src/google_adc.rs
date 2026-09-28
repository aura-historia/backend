use platform_google_adc::{MaterializationError, materialize_google_application_credentials};
use std::{
    env,
    ffi::OsStr,
    path::{Path, PathBuf},
};
use thiserror::Error;

const GOOGLE_ADC_CREDENTIALS_JSON_ENV: &str = "AURA_HISTORIA_GOOGLE_ADC_CREDENTIALS_JSON";
const GOOGLE_APPLICATION_CREDENTIALS_ENV: &str = "GOOGLE_APPLICATION_CREDENTIALS";
const GOOGLE_ADC_CREDENTIALS_DIRECTORY: &str = "/tmp/aura-historia-google-adc";

#[derive(Debug, Error, PartialEq, Eq)]
pub enum GoogleAdcError {
    #[error("invalid Google application credentials")]
    Invalid,
    #[error("failed to prepare private Google application credentials file")]
    File,
}

/// Call from synchronous `main`, before starting Tokio or any other worker threads.
pub fn materialize_google_application_credentials_from_env() -> Result<(), GoogleAdcError> {
    let injected_credentials = env::var_os(GOOGLE_ADC_CREDENTIALS_JSON_ENV);
    let selection = select_credentials(
        injected_credentials.as_deref(),
        Path::new(GOOGLE_ADC_CREDENTIALS_DIRECTORY),
    )?;
    if let Some(credentials_path) = selection {
        // SAFETY: synchronous main calls this before Tokio or any worker threads exist.
        unsafe {
            env::set_var(GOOGLE_APPLICATION_CREDENTIALS_ENV, credentials_path);
            env::remove_var(GOOGLE_ADC_CREDENTIALS_JSON_ENV);
        }
    }
    Ok(())
}

fn select_credentials(
    json: Option<&OsStr>,
    directory: &Path,
) -> Result<Option<PathBuf>, GoogleAdcError> {
    let Some(json) = json else {
        return Ok(None);
    };
    let json = json.to_str().ok_or(GoogleAdcError::Invalid)?;
    materialize_google_application_credentials(json, directory)
        .map(Some)
        .map_err(|error| match error {
            MaterializationError::Invalid => GoogleAdcError::Invalid,
            MaterializationError::File => GoogleAdcError::File,
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn injected_json_selects_materialized_adc() {
        let directory =
            env::temp_dir().join(format!("google-adc-periodic-valid-{}", std::process::id()));
        let json = r#"{"type":"authorized_user"}"#;
        let path = select_credentials(Some(OsStr::new(json)), &directory)
            .unwrap()
            .unwrap();
        assert_eq!(path, directory.join("application_default_credentials.json"));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), json);
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn absent_injection_preserves_local_adc_but_empty_injection_fails() {
        let directory = env::temp_dir().join(format!(
            "google-adc-periodic-selection-{}",
            std::process::id()
        ));
        assert_eq!(select_credentials(None, &directory), Ok(None));
        assert_eq!(
            select_credentials(Some(OsStr::new("")), &directory),
            Err(GoogleAdcError::Invalid)
        );
        for invalid in ["[]", "null", "not-json"] {
            assert_eq!(
                select_credentials(Some(OsStr::new(invalid)), &directory),
                Err(GoogleAdcError::Invalid)
            );
        }
        assert!(!directory.exists());
    }

    #[test]
    fn rejects_unwritable_credential_destination() {
        let directory =
            env::temp_dir().join(format!("google-adc-periodic-file-{}", std::process::id()));
        std::fs::write(&directory, "not a directory").unwrap();
        assert_eq!(
            select_credentials(Some(OsStr::new("{}")), &directory),
            Err(GoogleAdcError::File)
        );
        std::fs::remove_file(directory).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn non_unicode_injection_fails_instead_of_falling_back_to_local_adc() {
        use std::os::unix::ffi::OsStrExt;
        let directory = env::temp_dir().join(format!(
            "google-adc-periodic-nonunicode-{}",
            std::process::id()
        ));
        assert_eq!(
            select_credentials(Some(OsStr::from_bytes(b"\xff")), &directory),
            Err(GoogleAdcError::Invalid)
        );
        assert!(!directory.exists());
    }
}

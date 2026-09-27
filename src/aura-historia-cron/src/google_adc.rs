use std::{
    env,
    ffi::OsStr,
    fs::{self, OpenOptions},
    io::Write,
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
};
use thiserror::Error;

const GOOGLE_ADC_CREDENTIALS_JSON_ENV: &str = "AURA_HISTORIA_GOOGLE_ADC_CREDENTIALS_JSON";
const GOOGLE_APPLICATION_CREDENTIALS_ENV: &str = "GOOGLE_APPLICATION_CREDENTIALS";
const GOOGLE_ADC_CREDENTIALS_DIRECTORY: &str = "/tmp/aura-historia-google-adc";
const GOOGLE_ADC_CREDENTIALS_FILE_NAME: &str = "application_default_credentials.json";

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
    materialize_google_application_credentials(json, directory).map(Some)
}

fn materialize_google_application_credentials(
    credentials_json: &str,
    credentials_directory: &Path,
) -> Result<PathBuf, GoogleAdcError> {
    if !serde_json::from_str::<serde_json::Value>(credentials_json)
        .map(|value| value.is_object())
        .unwrap_or(false)
    {
        return Err(GoogleAdcError::Invalid);
    }
    fs::create_dir_all(credentials_directory).map_err(|_| GoogleAdcError::File)?;
    if !fs::symlink_metadata(credentials_directory)
        .map_err(|_| GoogleAdcError::File)?
        .file_type()
        .is_dir()
    {
        return Err(GoogleAdcError::File);
    }
    fs::set_permissions(credentials_directory, fs::Permissions::from_mode(0o700))
        .map_err(|_| GoogleAdcError::File)?;

    let credentials_path = credentials_directory.join(GOOGLE_ADC_CREDENTIALS_FILE_NAME);
    match fs::symlink_metadata(&credentials_path) {
        Ok(metadata) if !metadata.file_type().is_file() => return Err(GoogleAdcError::File),
        Ok(_) => (),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => (),
        Err(_) => return Err(GoogleAdcError::File),
    }
    let mut credentials_file = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(false)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(&credentials_path)
        .map_err(|_| GoogleAdcError::File)?;
    // An existing file might have more permissive modes. Restrict it before truncating or writing.
    credentials_file
        .set_permissions(fs::Permissions::from_mode(0o600))
        .and_then(|()| credentials_file.set_len(0))
        .and_then(|()| credentials_file.write_all(credentials_json.as_bytes()))
        .and_then(|()| credentials_file.sync_all())
        .map_err(|_| GoogleAdcError::File)?;
    Ok(credentials_path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static NEXT_DIRECTORY: AtomicUsize = AtomicUsize::new(0);

    fn test_directory() -> PathBuf {
        env::temp_dir().join(format!(
            "aura-historia-google-adc-bootstrap-{}-{}",
            std::process::id(),
            NEXT_DIRECTORY.fetch_add(1, Ordering::Relaxed)
        ))
    }

    #[test]
    fn materializes_private_credentials_and_restricts_existing_file_before_replacing() {
        let directory = test_directory();
        let path =
            materialize_google_application_credentials(r#"{"type":"service_account"}"#, &directory)
                .expect("materialize credentials");
        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            r#"{"type":"service_account"}"#
        );
        assert_eq!(
            fs::metadata(&directory).unwrap().permissions().mode() & 0o777,
            0o700
        );
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );

        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
        materialize_google_application_credentials(r#"{"type":"authorized_user"}"#, &directory)
            .expect("replace credentials");
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            r#"{"type":"authorized_user"}"#
        );
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn rejects_invalid_input_without_exposing_it_or_writing_a_file() {
        let directory = test_directory();
        let secret = "not-json-secret";
        for input in [secret, "[]", "null"] {
            let error = materialize_google_application_credentials(input, &directory).unwrap_err();
            assert_eq!(error, GoogleAdcError::Invalid);
            assert!(!error.to_string().contains(secret));
        }
        assert!(!directory.exists());
    }

    #[test]
    fn selection_preserves_local_adc_only_when_injection_absent() {
        let directory = test_directory();
        assert_eq!(select_credentials(None, &directory).unwrap(), None);
        assert_eq!(
            select_credentials(Some(OsStr::new("")), &directory).unwrap_err(),
            GoogleAdcError::Invalid
        );
        assert!(!directory.exists());
    }

    #[cfg(unix)]
    #[test]
    fn rejects_non_unicode_injected_json_instead_of_falling_back_to_local_adc() {
        use std::os::unix::ffi::OsStrExt;

        let directory = test_directory();
        assert_eq!(
            select_credentials(Some(OsStr::from_bytes(b"\xff")), &directory).unwrap_err(),
            GoogleAdcError::Invalid
        );
        assert!(!directory.exists());
    }

    #[test]
    fn rejects_symlinked_credentials_directory() {
        let target = test_directory();
        let directory = test_directory();
        fs::create_dir(&target).unwrap();
        fs::set_permissions(&target, fs::Permissions::from_mode(0o755)).unwrap();
        std::os::unix::fs::symlink(&target, &directory).unwrap();
        assert_eq!(
            materialize_google_application_credentials("{}", &directory).unwrap_err(),
            GoogleAdcError::File
        );
        assert_eq!(
            fs::metadata(&target).unwrap().permissions().mode() & 0o777,
            0o755
        );
        fs::remove_file(directory).unwrap();
        fs::remove_dir(target).unwrap();
    }

    #[test]
    fn rejects_symlink_instead_of_writing_credentials_through_it() {
        let directory = test_directory();
        fs::create_dir(&directory).unwrap();
        let target = directory.join("target");
        fs::write(&target, "untouched").unwrap();
        std::os::unix::fs::symlink(&target, directory.join(GOOGLE_ADC_CREDENTIALS_FILE_NAME))
            .unwrap();
        let error = materialize_google_application_credentials("{}", &directory).unwrap_err();
        assert_eq!(error, GoogleAdcError::File);
        assert_eq!(fs::read_to_string(target).unwrap(), "untouched");
        fs::remove_dir_all(directory).unwrap();
    }
}

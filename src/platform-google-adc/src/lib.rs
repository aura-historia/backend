//! Private, runtime-independent materialization of injected Google ADC JSON.

use std::{
    fs::{self, File, OpenOptions},
    io::Write,
    os::{
        fd::AsRawFd,
        fd::FromRawFd,
        unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt},
    },
    path::{Path, PathBuf},
};
use thiserror::Error;

const CREDENTIALS_FILE_NAME: &str = "application_default_credentials.json";
const CREDENTIALS_FILE_NAME_C: &[u8] = b"application_default_credentials.json\0";

#[derive(Debug, Error, PartialEq, Eq)]
pub enum MaterializationError {
    #[error("invalid Google application credentials")]
    Invalid,
    #[error("failed to prepare private Google application credentials file")]
    File,
}

/// Materializes a JSON object in a private directory. The parent of `directory` must be
/// a trusted location (for example `/tmp`); this function never follows a symlink at
/// the directory or credentials-file leaf. No credential contents or I/O paths escape in errors.
pub fn materialize_google_application_credentials(
    credentials_json: &str,
    directory: &Path,
) -> Result<PathBuf, MaterializationError> {
    if !serde_json::from_str::<serde_json::Value>(credentials_json)
        .map(|value| value.is_object())
        .unwrap_or(false)
    {
        return Err(MaterializationError::Invalid);
    }

    match fs::create_dir(directory) {
        Ok(()) => (),
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => (),
        Err(_) => return Err(MaterializationError::File),
    }
    // Pin the directory before changing permissions or opening its child. O_NOFOLLOW
    // and O_DIRECTORY exclude a symlink (including a dangling one) at the leaf.
    let directory_file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(directory)
        .map_err(|_| MaterializationError::File)?;
    let metadata = directory_file
        .metadata()
        .map_err(|_| MaterializationError::File)?;
    if !metadata.is_dir() || metadata.uid() != unsafe { libc::geteuid() } {
        return Err(MaterializationError::File);
    }
    directory_file
        .set_permissions(fs::Permissions::from_mode(0o700))
        .map_err(|_| MaterializationError::File)?;

    // openat keeps the child relative to the pinned directory. O_NOFOLLOW prevents
    // replacing the file with a symlink between inspection and open; O_NONBLOCK avoids
    // blocking on an unexpected FIFO. Check type, owner and link count before writing.
    let fd = unsafe {
        libc::openat(
            directory_file.as_raw_fd(),
            CREDENTIALS_FILE_NAME_C.as_ptr().cast(),
            libc::O_WRONLY | libc::O_CREAT | libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC,
            0o600,
        )
    };
    if fd < 0 {
        return Err(MaterializationError::File);
    }
    // SAFETY: openat returned a new, owned file descriptor on success.
    let mut file = unsafe { File::from_raw_fd(fd) };
    let metadata = file.metadata().map_err(|_| MaterializationError::File)?;
    if !metadata.is_file() || metadata.nlink() != 1 || metadata.uid() != unsafe { libc::geteuid() }
    {
        return Err(MaterializationError::File);
    }
    // Existing files may be too permissive; tighten before replacing any contents.
    file.set_permissions(fs::Permissions::from_mode(0o600))
        .and_then(|()| file.set_len(0))
        .and_then(|()| file.write_all(credentials_json.as_bytes()))
        .and_then(|()| file.sync_all())
        .map_err(|_| MaterializationError::File)?;
    Ok(directory.join(CREDENTIALS_FILE_NAME))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static NEXT_DIRECTORY: AtomicUsize = AtomicUsize::new(0);

    fn test_directory() -> PathBuf {
        std::env::temp_dir().join(format!(
            "aura-historia-google-adc-shared-{}-{}",
            std::process::id(),
            NEXT_DIRECTORY.fetch_add(1, Ordering::Relaxed)
        ))
    }

    #[test]
    fn creates_private_file_and_restricts_existing_file_before_replacing() {
        let directory = test_directory();
        let path =
            materialize_google_application_credentials(r#"{"type":"service_account"}"#, &directory)
                .expect("create credentials");
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

        fs::set_permissions(&directory, fs::Permissions::from_mode(0o755)).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
        materialize_google_application_credentials(r#"{"type":"authorized_user"}"#, &directory)
            .expect("replace credentials");
        assert_eq!(
            fs::metadata(&directory).unwrap().permissions().mode() & 0o777,
            0o700
        );
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
    fn rejects_invalid_input_without_writing_or_disclosing_it() {
        let directory = test_directory();
        for input in ["", "not-json-secret", "[]", "null", "true", "\"string\""] {
            let error = materialize_google_application_credentials(input, &directory).unwrap_err();
            assert_eq!(error, MaterializationError::Invalid);
            if !input.is_empty() {
                assert!(!format!("{error:?} {error}").contains(input));
            }
        }
        assert!(!directory.exists());
    }

    #[test]
    fn rejects_symlinked_directory_without_changing_target() {
        let target = test_directory();
        let directory = test_directory();
        fs::create_dir(&target).unwrap();
        fs::set_permissions(&target, fs::Permissions::from_mode(0o755)).unwrap();
        std::os::unix::fs::symlink(&target, &directory).unwrap();
        assert_eq!(
            materialize_google_application_credentials("{}", &directory),
            Err(MaterializationError::File)
        );
        assert_eq!(
            fs::metadata(&target).unwrap().permissions().mode() & 0o777,
            0o755
        );
        fs::remove_file(directory).unwrap();
        fs::remove_dir(target).unwrap();
    }

    #[test]
    fn rejects_symlinked_or_hard_linked_file_without_touching_target() {
        let directory = test_directory();
        fs::create_dir(&directory).unwrap();
        let target = directory.join("target");
        fs::write(&target, "untouched").unwrap();
        let path = directory.join(CREDENTIALS_FILE_NAME);
        std::os::unix::fs::symlink(&target, &path).unwrap();
        assert_eq!(
            materialize_google_application_credentials("{}", &directory),
            Err(MaterializationError::File)
        );
        fs::remove_file(&path).unwrap();
        fs::hard_link(&target, &path).unwrap();
        assert_eq!(
            materialize_google_application_credentials("{}", &directory),
            Err(MaterializationError::File)
        );
        assert_eq!(fs::read_to_string(target).unwrap(), "untouched");
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn rejects_non_regular_destination() {
        let directory = test_directory();
        fs::create_dir(&directory).unwrap();
        fs::create_dir(directory.join(CREDENTIALS_FILE_NAME)).unwrap();
        assert_eq!(
            materialize_google_application_credentials("{}", &directory),
            Err(MaterializationError::File)
        );
        fs::remove_dir_all(directory).unwrap();
    }
}

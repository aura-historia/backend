//! Configuration from environment variables and from files.
//!
//! Every setting is an environment variable. A secret may instead come from a file: when
//! `NAME_FILE` is set, [`load_file_backed_environment`] reads that file into `NAME` once at
//! startup, so the rest of the process reads `NAME` as before. Setting both is an error, as is a
//! file that cannot be read. Values are never logged or echoed in errors. Variables that other
//! tools define with a `_FILE` suffix of their own, such as `AWS_CONFIG_FILE`, `SSL_CERT_FILE` or
//! `LLVM_PROFILE_FILE`, keep their meaning and are left alone.

use std::{ffi::OsString, path::PathBuf, str::FromStr};

const FILE_SUFFIX: &str = "_FILE";
/// Prefixes of tools whose own variables end in `_FILE`; they are never secret indirections.
const FOREIGN_PREFIXES: [&str; 8] = [
    "AWS_", "SSL_", "LLVM_", "CARGO_", "RUSTUP_", "CURL_", "GOOGLE_", "NEXTEST_",
];

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("missing required configuration {name}")]
    Missing { name: String },
    #[error("invalid configuration {name}")]
    Invalid { name: String },
    #[error("configuration {name} is set both directly and through {name}_FILE")]
    Ambiguous { name: String },
    #[error("cannot read the file configured by {name}_FILE")]
    File {
        name: String,
        #[source]
        source: std::io::Error,
    },
}

/// Reads every `NAME_FILE` into `NAME`. A single trailing line break is removed, so files written
/// with a newline hold the same value as the variable would.
///
/// Call it from synchronous `main`, before the async runtime or any other thread starts: it
/// changes the process environment.
pub fn load_file_backed_environment() -> Result<(), ConfigError> {
    let resolved = resolve_file_backed(std::env::vars_os(), |path| std::fs::read(path))?;
    for (name, value) in resolved {
        // SAFETY: callers run this in synchronous process initialization before spawning threads.
        unsafe { std::env::set_var(name, value) };
    }
    Ok(())
}

fn resolve_file_backed(
    variables: impl IntoIterator<Item = (OsString, OsString)>,
    mut read: impl FnMut(&PathBuf) -> std::io::Result<Vec<u8>>,
) -> Result<Vec<(String, String)>, ConfigError> {
    let variables: Vec<(OsString, OsString)> = variables.into_iter().collect();
    let is_set = |name: &str| variables.iter().any(|(key, _)| key == name);
    let mut resolved = Vec::new();
    for (key, path) in &variables {
        let Some(name) = key.to_str().and_then(|key| key.strip_suffix(FILE_SUFFIX)) else {
            continue;
        };
        if name.is_empty()
            || FOREIGN_PREFIXES
                .iter()
                .any(|prefix| name.starts_with(prefix))
        {
            continue;
        }
        if is_set(name) {
            return Err(ConfigError::Ambiguous { name: name.into() });
        }
        let path = PathBuf::from(path);
        let bytes = read(&path).map_err(|source| ConfigError::File {
            name: name.into(),
            source,
        })?;
        let value = String::from_utf8(bytes).map_err(|_| ConfigError::Invalid {
            name: format!("{name}{FILE_SUFFIX}"),
        })?;
        let value = value
            .strip_suffix("\r\n")
            .or_else(|| value.strip_suffix('\n'))
            .unwrap_or(&value)
            .to_owned();
        if value.is_empty() || value.contains('\0') {
            return Err(ConfigError::Invalid {
                name: format!("{name}{FILE_SUFFIX}"),
            });
        }
        resolved.push((name.to_owned(), value));
    }
    Ok(resolved)
}

/// A non-blank setting.
pub fn required(name: &str) -> Result<String, ConfigError> {
    match std::env::var(name) {
        Ok(value) if !value.trim().is_empty() => Ok(value),
        Ok(_) | Err(std::env::VarError::NotPresent) => {
            Err(ConfigError::Missing { name: name.into() })
        }
        Err(std::env::VarError::NotUnicode(_)) => Err(ConfigError::Invalid { name: name.into() }),
    }
}

/// A setting parsed as `T`, or `default` when it is absent. A present but unparsable value is an
/// error, never the default.
pub fn optional<T: FromStr>(name: &str, default: T) -> Result<T, ConfigError> {
    match std::env::var(name) {
        Ok(value) => value
            .trim()
            .parse()
            .map_err(|_| ConfigError::Invalid { name: name.into() }),
        Err(std::env::VarError::NotPresent) => Ok(default),
        Err(std::env::VarError::NotUnicode(_)) => Err(ConfigError::Invalid { name: name.into() }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn vars(pairs: &[(&str, &str)]) -> Vec<(OsString, OsString)> {
        pairs
            .iter()
            .map(|(key, value)| (OsString::from(key), OsString::from(value)))
            .collect()
    }

    #[test]
    fn should_read_a_file_backed_secret_without_its_trailing_newline() {
        let files = HashMap::from([(PathBuf::from("/run/secrets/pg"), b"s3cret\n".to_vec())]);

        let resolved = resolve_file_backed(
            vars(&[
                ("POSTGRES_PASSWORD_FILE", "/run/secrets/pg"),
                ("STAGE", "prod"),
            ]),
            |path| Ok(files[path].clone()),
        )
        .unwrap();

        assert_eq!(
            vec![("POSTGRES_PASSWORD".to_owned(), "s3cret".to_owned())],
            resolved
        );
    }

    #[test]
    fn should_leave_other_tools_file_variables_alone() {
        let resolved = resolve_file_backed(
            vars(&[
                ("AWS_CONFIG_FILE", "/dev/null"),
                ("AWS_WEB_IDENTITY_TOKEN_FILE", "/var/run/token"),
                ("SSL_CERT_FILE", "/etc/ssl/cert.pem"),
                ("LLVM_PROFILE_FILE", ""),
            ]),
            |_| panic!("no foreign variable is read"),
        )
        .unwrap();

        assert!(resolved.is_empty());
    }

    #[test]
    fn should_reject_a_setting_given_both_directly_and_through_a_file() {
        let error = resolve_file_backed(
            vars(&[
                ("POSTGRES_PASSWORD", "direct"),
                ("POSTGRES_PASSWORD_FILE", "/run/secrets/pg"),
            ]),
            |_| Ok(b"file".to_vec()),
        )
        .unwrap_err();

        assert!(matches!(error, ConfigError::Ambiguous { name } if name == "POSTGRES_PASSWORD"));
    }

    #[test]
    fn should_fail_closed_on_unreadable_empty_or_binary_files_without_echoing_them() {
        let missing = resolve_file_backed(vars(&[("TOKEN_FILE", "/missing")]), |_| {
            Err(std::io::Error::from(std::io::ErrorKind::NotFound))
        })
        .unwrap_err();
        assert!(matches!(missing, ConfigError::File { .. }));

        for content in [&b"\n"[..], &b"\xff\xfe"[..], &b"a\0b"[..]] {
            let error =
                resolve_file_backed(vars(&[("TOKEN_FILE", "/t")]), |_| Ok(content.to_vec()))
                    .unwrap_err();
            assert!(matches!(error, ConfigError::Invalid { name } if name == "TOKEN_FILE"));
        }
    }
}

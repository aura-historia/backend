use super::{Result, failure};

pub(super) fn resource(
    run: &str,
    test: &str,
    label: &str,
    maximum: usize,
    suffix: &str,
) -> Result<String> {
    if label.is_empty()
        || !label
            .bytes()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-')
        || label.starts_with('-')
        || label.ends_with('-')
    {
        return Err(failure(
            "resource label requires lowercase ASCII letters, digits and internal hyphens",
        ));
    }
    let prefix = format!("a{run}-{}-", &test[..16]);
    if prefix.len() + label.len() + suffix.len() > maximum {
        return Err(failure(
            "scoped resource name exceeds the service limit; use a shorter label",
        ));
    }
    Ok(format!("{prefix}{label}{suffix}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn should_preserve_fifo_suffix_and_bound_bucket_queue_and_ses_names() {
        let run = "0".repeat(32);
        let test = "1".repeat(32);
        assert_eq!(
            80,
            resource(&run, &test, &"q".repeat(24), 80, ".fifo")
                .unwrap()
                .len()
        );
        assert!(resource(&run, &test, &"q".repeat(25), 80, ".fifo").is_err());
        assert_eq!(
            63,
            resource(&run, &test, &"b".repeat(12), 63, "")
                .unwrap()
                .len()
        );
        assert!(resource(&run, &test, "bucket.too-long", 63, "").is_err());
        assert!(resource(&run, &test, "-invalid", 64, "").is_err());
        assert_eq!(
            64,
            resource(&run, &test, &"s".repeat(13), 64, "")
                .unwrap()
                .len()
        );
        assert!(resource(&run, &test, &"s".repeat(14), 64, "").is_err());
        assert!(resource(&run, &test, "sender@example.test", 64, "").is_err());
    }
}

use platform_postgres_queue::ErrorClass;

const FALLBACK: &str = "unclassified";
const MAX_BYTES: usize = 64;

/// The queue's bounded error class for a handler's static outcome label, such as `timed_out` or
/// a service diagnostic code like `UPDATE_NOT_FOUND`. Labels are lower-cased, characters outside
/// the class alphabet become underscores and long labels are cut, so a label never fails a
/// settlement; an empty label becomes `unclassified`.
pub fn error_class(label: &str) -> ErrorClass {
    let normalized: String = label
        .chars()
        .map(|c| match c.to_ascii_lowercase() {
            c @ ('a'..='z' | '0'..='9' | '_') => c,
            _ => '_',
        })
        .take(MAX_BYTES)
        .collect();
    ErrorClass::new(normalized)
        .unwrap_or_else(|_| ErrorClass::new(FALLBACK).expect("the fallback error class is valid"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_map_any_static_label_to_a_valid_error_class() {
        for (label, expected) in [
            ("timed_out", "timed_out"),
            ("UPDATE_NOT_FOUND", "update_not_found"),
            ("invalid-wire job", "invalid_wire_job"),
            ("", "unclassified"),
        ] {
            assert_eq!(expected, error_class(label).as_str());
        }
        assert_eq!(64, error_class(&"x".repeat(100)).as_str().len());
    }
}

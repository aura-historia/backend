//! Opt-in measurements for CI baselines, independent of tracing subscribers.

use std::io::Write;
use std::time::Duration;

/// Emits an observation only when the benchmark collector explicitly enables it.
/// Measurement output must never affect a fixture or a test, including on a closed pipe.
#[doc(hidden)]
pub fn record_timing(phase: &str, owner: &str, elapsed: Duration) {
    write_timing(
        &mut std::io::stderr().lock(),
        std::env::var("AURA_CI_BENCHMARK_TIMING").ok().as_deref(),
        phase,
        owner,
        elapsed,
    );
}

fn write_timing(
    writer: &mut impl Write,
    setting: Option<&str>,
    phase: &str,
    owner: &str,
    elapsed: Duration,
) {
    if setting != Some("1") {
        return;
    }
    let record = serde_json::json!({
        "phase": phase,
        "owner": owner,
        "pid": std::process::id(),
        "elapsed_seconds": elapsed.as_secs_f64(),
    });
    let _ = writeln!(writer, "AURA_CI_TIMING {record}");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_emit_parseable_record_when_benchmark_timing_is_enabled() {
        let mut output = Vec::new();
        write_timing(
            &mut output,
            Some("1"),
            "BODY",
            "example::test",
            Duration::from_millis(125),
        );
        let output = String::from_utf8(output).unwrap();
        let record: serde_json::Value =
            serde_json::from_str(output.strip_prefix("AURA_CI_TIMING ").unwrap()).unwrap();
        assert_eq!(record["phase"], "BODY");
        assert_eq!(record["owner"], "example::test");
        assert_eq!(record["pid"], std::process::id());
        assert_eq!(record["elapsed_seconds"], 0.125);
    }

    #[test]
    fn should_emit_nothing_when_setting_is_absent_disabled_or_malformed() {
        for setting in [None, Some("0"), Some("true"), Some(" 1"), Some("")] {
            let mut output = Vec::new();
            write_timing(&mut output, setting, "BODY", "example", Duration::ZERO);
            assert!(output.is_empty(), "setting {setting:?} must not enable timing");
        }
    }

    #[test]
    fn should_preserve_json_escaping_when_owner_contains_special_characters() {
        let mut output = Vec::new();
        let owner = "example::\"quoted\"\nname";
        write_timing(&mut output, Some("1"), "BODY", owner, Duration::ZERO);
        let output = String::from_utf8(output).unwrap();
        assert_eq!(output.lines().count(), 1);
        let record: serde_json::Value =
            serde_json::from_str(output.strip_prefix("AURA_CI_TIMING ").unwrap()).unwrap();
        assert_eq!(record["owner"], owner);
    }

    #[test]
    fn should_preserve_test_execution_when_measurement_output_fails() {
        struct ClosedPipe;
        impl Write for ClosedPipe {
            fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
                Err(std::io::ErrorKind::BrokenPipe.into())
            }

            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        write_timing(
            &mut ClosedPipe,
            Some("1"),
            "BODY",
            "example",
            Duration::ZERO,
        );
    }
}

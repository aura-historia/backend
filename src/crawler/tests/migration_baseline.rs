static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("./migrations");
const ADOPTION_SQL: &str = include_str!("../scripts/adopt-squashed-baseline.sql");

#[test]
fn adoption_script_records_the_embedded_baseline_checksum() {
    let baseline = MIGRATOR
        .iter()
        .find(|migration| migration.version == 20260101000000)
        .expect("squashed crawler baseline is embedded");
    let checksum: String = baseline
        .checksum
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();

    // Only the adoption target constant ends with `';`; pre-squash entries do not.
    assert!(
        ADOPTION_SQL.contains(&format!("'{checksum}';")),
        "the applied baseline changed; never edit it, or adopted ledgers fail at startup"
    );
}

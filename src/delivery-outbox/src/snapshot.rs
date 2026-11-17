//! Strict codecs for outbox snapshots. Each selected source has one versioned shape: exactly
//! the columns its route reads, with their JSON types. Anything else fails closed.

use aura_historia_cdc_routing::{CdcChange, Operation};
use serde_json::{Map, Value};
use std::collections::BTreeMap;

/// The only snapshot shape the capture triggers write.
pub const SNAPSHOT_VERSION: i16 = 1;

/// A table whose committed inserts the outbox captures.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OutboxSource {
    ProductListingEvents,
    ProductListingRawRevisions,
    SearchFilterMatches,
    NotificationDeliveries,
    MarketingEmailConsentSyncIntents,
    GeoResolutionWakeups,
    ListingGeographyWakeups,
    ListingShippingWakeups,
    UserIdentityDeletionRequests,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ColumnKind {
    /// A UUID, captured as its canonical text.
    Uuid,
    Text,
    Integer,
    Object,
}

impl ColumnKind {
    fn admits(self, value: &Value) -> bool {
        match self {
            Self::Uuid | Self::Text => value.is_string(),
            Self::Integer => value.as_i64().is_some(),
            Self::Object => value.is_object(),
        }
    }
}

impl OutboxSource {
    pub const ALL: [Self; 9] = [
        Self::ProductListingEvents,
        Self::ProductListingRawRevisions,
        Self::SearchFilterMatches,
        Self::NotificationDeliveries,
        Self::MarketingEmailConsentSyncIntents,
        Self::GeoResolutionWakeups,
        Self::ListingGeographyWakeups,
        Self::ListingShippingWakeups,
        Self::UserIdentityDeletionRequests,
    ];

    pub const fn table(self) -> &'static str {
        match self {
            Self::ProductListingEvents => "product_listing_events",
            Self::ProductListingRawRevisions => "product_listing_raw_revisions",
            Self::SearchFilterMatches => "search_filter_matches",
            Self::NotificationDeliveries => "notification_deliveries",
            Self::MarketingEmailConsentSyncIntents => "marketing_email_consent_sync_intents",
            Self::GeoResolutionWakeups => "geo_resolution_wakeups",
            Self::ListingGeographyWakeups => "listing_geography_wakeups",
            Self::ListingShippingWakeups => "listing_shipping_wakeups",
            Self::UserIdentityDeletionRequests => "user_identity_deletion_requests",
        }
    }

    pub fn from_table(table: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|source| source.table() == table)
    }

    /// The version-1 columns, mirroring the capture trigger in
    /// `migrations/20261118090000_delivery_outbox.sql`.
    const fn columns(self) -> &'static [(&'static str, ColumnKind)] {
        use ColumnKind::{Integer, Object, Text, Uuid};
        match self {
            Self::ProductListingEvents => &[
                ("event_id", Uuid),
                ("product_listing_id", Uuid),
                ("event_type", Text),
                ("event_group", Text),
                ("event_type_schema_version", Integer),
                ("payload", Object),
            ],
            Self::ProductListingRawRevisions => &[
                ("product_listing_raw_stream_id", Uuid),
                ("product_listing_raw_revision_id", Uuid),
                ("revision", Integer),
            ],
            Self::SearchFilterMatches => &[
                ("user_id", Uuid),
                ("user_search_filter_id", Uuid),
                ("product_listing_id", Uuid),
                ("origin_event_id", Uuid),
            ],
            Self::NotificationDeliveries => &[("notification_delivery_id", Uuid)],
            Self::MarketingEmailConsentSyncIntents => {
                &[("intent_id", Uuid), ("recipient_key", Text)]
            }
            Self::GeoResolutionWakeups => &[("resolution_id", Uuid), ("generation", Integer)],
            Self::ListingGeographyWakeups | Self::ListingShippingWakeups => {
                &[("work_id", Uuid), ("generation", Integer)]
            }
            Self::UserIdentityDeletionRequests => &[("user_id", Uuid)],
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum SnapshotError {
    #[error("outbox snapshot comes from a table the outbox does not capture")]
    UnsupportedSource,
    #[error("outbox snapshot has an unsupported version")]
    UnsupportedVersion,
    #[error("outbox snapshot does not have its source's columns")]
    UnexpectedColumns,
    #[error("outbox snapshot column {0} has the wrong type")]
    InvalidColumn(&'static str),
}

impl SnapshotError {
    /// The bounded failure label recorded in custody.
    pub const fn class(self) -> &'static str {
        match self {
            Self::UnsupportedSource => "unsupported_source",
            Self::UnsupportedVersion => "unsupported_snapshot_version",
            Self::UnexpectedColumns | Self::InvalidColumn(_) => "invalid_snapshot",
        }
    }
}

/// Decodes a captured snapshot into the committed insert that routing consumes. The snapshot
/// must carry exactly its source's columns with their types; routing then validates their
/// values.
pub fn decode(
    source_table: &str,
    version: i16,
    snapshot: Value,
) -> Result<CdcChange, SnapshotError> {
    let source = OutboxSource::from_table(source_table).ok_or(SnapshotError::UnsupportedSource)?;
    if version != SNAPSHOT_VERSION {
        return Err(SnapshotError::UnsupportedVersion);
    }
    let Value::Object(row) = snapshot else {
        return Err(SnapshotError::UnexpectedColumns);
    };
    validate_columns(source, &row)?;
    Ok(CdcChange {
        schema: Some("public".to_owned()),
        table: source.table().to_owned(),
        operation: Operation::Insert,
        primary_key: BTreeMap::new(),
        record: Some(Value::Object(row)),
        old_record: None,
        changed_columns: Vec::new(),
        commit_lsn: None,
        commit_timestamp: None,
    })
}

fn validate_columns(source: OutboxSource, row: &Map<String, Value>) -> Result<(), SnapshotError> {
    let columns = source.columns();
    if row.len() != columns.len() {
        return Err(SnapshotError::UnexpectedColumns);
    }
    for (name, kind) in columns {
        let value = row.get(*name).ok_or(SnapshotError::UnexpectedColumns)?;
        if !kind.admits(value) {
            return Err(SnapshotError::InvalidColumn(name));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn wakeup() -> Value {
        json!({"work_id": "0194f1c2-5b6a-7c3d-8e9f-0a1b2c3d4e5f", "generation": 3})
    }

    #[test]
    fn should_decode_a_snapshot_into_its_committed_insert() {
        let change = decode("listing_shipping_wakeups", SNAPSHOT_VERSION, wakeup()).unwrap();

        assert_eq!("listing_shipping_wakeups", change.table);
        assert_eq!(Some("public"), change.schema.as_deref());
        assert_eq!(Operation::Insert, change.operation);
        assert_eq!(Some(wakeup()), change.record);
        assert_eq!(None, change.old_record);
    }

    #[test]
    fn should_know_every_captured_table_by_name() {
        for source in OutboxSource::ALL {
            assert_eq!(Some(source), OutboxSource::from_table(source.table()));
        }
        for table in ["product_listings", "users", "product_listing_watchlist", ""] {
            assert_eq!(None, OutboxSource::from_table(table), "{table}");
        }
    }

    #[test]
    fn should_reject_a_source_the_outbox_does_not_capture() {
        assert_eq!(
            Err(SnapshotError::UnsupportedSource),
            decode("product_listings", SNAPSHOT_VERSION, wakeup())
        );
    }

    #[test]
    fn should_reject_an_unknown_snapshot_version() {
        for version in [0, 2, -1] {
            assert_eq!(
                Err(SnapshotError::UnsupportedVersion),
                decode("listing_shipping_wakeups", version, wakeup())
            );
        }
    }

    #[test]
    fn should_reject_extra_missing_or_renamed_columns() {
        let id = "0194f1c2-5b6a-7c3d-8e9f-0a1b2c3d4e5f";
        for snapshot in [
            json!({"work_id": id, "generation": 3, "pending": true}),
            json!({"work_id": id}),
            json!({"work_id": id, "revision": 3}),
            json!([id, 3]),
            json!(null),
        ] {
            assert_eq!(
                Err(SnapshotError::UnexpectedColumns),
                decode(
                    "listing_shipping_wakeups",
                    SNAPSHOT_VERSION,
                    snapshot.clone()
                ),
                "{snapshot}"
            );
        }
    }

    #[test]
    fn should_reject_columns_of_the_wrong_type() {
        let id = "0194f1c2-5b6a-7c3d-8e9f-0a1b2c3d4e5f";
        for (snapshot, column) in [
            (json!({"work_id": 7, "generation": 3}), "work_id"),
            (json!({"work_id": id, "generation": "3"}), "generation"),
            (json!({"work_id": id, "generation": 3.5}), "generation"),
        ] {
            assert_eq!(
                Err(SnapshotError::InvalidColumn(column)),
                decode("listing_shipping_wakeups", SNAPSHOT_VERSION, snapshot)
            );
        }
        let event = json!({
            "event_id": id, "product_listing_id": id, "event_type": "ENRICHMENT_EMBEDDED",
            "event_group": "ENRICHMENT", "event_type_schema_version": 1, "payload": "{}"
        });
        assert_eq!(
            Err(SnapshotError::InvalidColumn("payload")),
            decode("product_listing_events", SNAPSHOT_VERSION, event)
        );
    }

    #[test]
    fn should_admit_only_the_two_consent_columns() {
        let id = "0194f1c2-5b6a-7c3d-8e9f-0a1b2c3d4e5f";
        let key = "a".repeat(64);

        assert!(
            decode(
                "marketing_email_consent_sync_intents",
                SNAPSHOT_VERSION,
                json!({"intent_id": id, "recipient_key": key})
            )
            .is_ok()
        );
        assert_eq!(
            Err(SnapshotError::UnexpectedColumns),
            decode(
                "marketing_email_consent_sync_intents",
                SNAPSHOT_VERSION,
                json!({"intent_id": id, "recipient_key": key, "email": "a@example.com"})
            )
        );
    }
}

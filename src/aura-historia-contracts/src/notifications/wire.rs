use serde::Serializer;

pub(super) mod kind {
    use super::*;
    use notification_core::notification_kind::NotificationKind;
    use strum::IntoEnumIterator;
    pub fn schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
        schemars::json_schema!({"type":"string","enum":NotificationKind::iter().map(NotificationKind::as_str).collect::<Vec<_>>()})
    }

    pub fn serialize<S>(value: &NotificationKind, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(value.as_str())
    }
}

pub(super) mod decision {
    use super::*;
    use notification_core::notification::PartnershipApplicationDecision;

    pub fn schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
        schemars::json_schema!({"type":"string","enum":["APPROVED","REJECTED"]})
    }

    fn code(value: PartnershipApplicationDecision) -> &'static str {
        match value {
            PartnershipApplicationDecision::Approved => "APPROVED",
            PartnershipApplicationDecision::Rejected => "REJECTED",
        }
    }

    pub fn serialize<S>(
        value: &PartnershipApplicationDecision,
        serializer: S,
    ) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(code(*value))
    }
}

pub(super) fn partnership_application_id_schema(
    _: &mut schemars::SchemaGenerator,
) -> schemars::Schema {
    crate::wire::schema::object_id(
        partnership_core::partnership_application_id::PartnershipApplicationId::PREFIX,
    )
}

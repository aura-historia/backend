use serde::Serializer;

pub(super) mod kind {
    use super::*;
    use notification_core::notification_kind::NotificationKind;

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

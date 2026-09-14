use crate::{
    error::{ApiError, BAD_BODY_VALUE},
    patch_value::{PatchValue, clearable},
    values::LocalizedTextData,
    wire::parse_body_object_id,
};
use application::patch_field::PatchField;
use auction_core::{
    AuctionDescription, AuctionFormat, AuctionId, AuctionName, AuctionReportedStatus,
    AuctionSchedule, ReportedCatalogueLotCount, SourceAuctionId,
};
use auction_service::{
    ports::AuctionStorageVersion,
    use_cases::{
        commands::{
            create_auction::CreateAuctionCommand,
            update_auction::{AuctionSchedulePatch, UpdateAuctionCommand},
        },
        queries::get_auction::AuctionAdminDetailsView,
    },
};
use listing_source_core::ListingSourceId;
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use url::Url;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct CreateAuctionData {
    listing_source_id: String,
    source_auction_id: String,
    #[serde(default)]
    name: Option<LocalizedTextData>,
    #[serde(default)]
    description: Option<LocalizedTextData>,
    #[serde(default)]
    catalogue_url: Option<Url>,
    #[serde(default)]
    format: Option<String>,
    #[serde(default)]
    schedule: AuctionScheduleData,
    #[serde(default)]
    reported_status: Option<String>,
    #[serde(default)]
    reported_lot_count: Option<u32>,
}

impl TryFrom<CreateAuctionData> for CreateAuctionCommand {
    type Error = ApiError;

    fn try_from(value: CreateAuctionData) -> Result<Self, Self::Error> {
        Ok(Self {
            listing_source_id: parse_body_object_id(
                &value.listing_source_id,
                "listingSourceId",
                "ListingSource",
            )?,
            source_auction_id: source_auction_id(value.source_auction_id)?,
            name: value.name.map(auction_name).transpose()?,
            description: value.description.map(auction_description).transpose()?,
            catalogue_url: value.catalogue_url,
            format: value.format.map(auction_format).transpose()?,
            schedule: value.schedule.into_schedule()?,
            reported_status: value.reported_status.map(auction_status).transpose()?,
            reported_lot_count: value.reported_lot_count.map(ReportedCatalogueLotCount::new),
        })
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct UpdateAuctionData {
    expected_version: u64,
    #[serde(default)]
    name: PatchValue<LocalizedTextData>,
    #[serde(default)]
    description: PatchValue<LocalizedTextData>,
    #[serde(default)]
    catalogue_url: PatchValue<Url>,
    #[serde(default)]
    format: PatchValue<String>,
    #[serde(default)]
    schedule: AuctionSchedulePatchData,
    #[serde(default)]
    reported_status: PatchValue<String>,
    #[serde(default)]
    reported_lot_count: PatchValue<u32>,
}

impl UpdateAuctionData {
    pub(super) fn into_command(
        self,
        auction_id: AuctionId,
    ) -> Result<UpdateAuctionCommand, ApiError> {
        let expected_version = AuctionStorageVersion::try_from(self.expected_version)
            .map_err(|_| invalid_body("expectedVersion must be a positive integer."))?;
        Ok(UpdateAuctionCommand {
            auction_id,
            expected_version,
            name: map_patch(self.name, auction_name)?,
            description: map_patch(self.description, auction_description)?,
            catalogue_url: clearable(self.catalogue_url),
            format: map_patch(self.format, auction_format)?,
            schedule: self.schedule.into_patch(),
            reported_status: map_patch(self.reported_status, auction_status)?,
            reported_lot_count: map_patch(self.reported_lot_count, |value| {
                Ok(ReportedCatalogueLotCount::new(value))
            })?,
        })
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct AuctionScheduleData {
    #[serde(default, with = "time::serde::rfc3339::option")]
    bidding_opens: Option<OffsetDateTime>,
    #[serde(default, with = "time::serde::rfc3339::option")]
    live_starts: Option<OffsetDateTime>,
    #[serde(default, with = "time::serde::rfc3339::option")]
    lots_begin_closing: Option<OffsetDateTime>,
    #[serde(default, with = "time::serde::rfc3339::option")]
    scheduled_end: Option<OffsetDateTime>,
}

impl AuctionScheduleData {
    fn into_schedule(self) -> Result<AuctionSchedule, ApiError> {
        AuctionSchedule::new(
            self.bidding_opens,
            self.live_starts,
            self.lots_begin_closing,
            self.scheduled_end,
        )
        .map_err(|_| invalid_body("schedule has invalid bounds."))
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct AuctionSchedulePatchData {
    #[serde(default, deserialize_with = "crate::patch_value::rfc3339::deserialize")]
    bidding_opens: PatchValue<OffsetDateTime>,
    #[serde(default, deserialize_with = "crate::patch_value::rfc3339::deserialize")]
    live_starts: PatchValue<OffsetDateTime>,
    #[serde(default, deserialize_with = "crate::patch_value::rfc3339::deserialize")]
    lots_begin_closing: PatchValue<OffsetDateTime>,
    #[serde(default, deserialize_with = "crate::patch_value::rfc3339::deserialize")]
    scheduled_end: PatchValue<OffsetDateTime>,
}

impl AuctionSchedulePatchData {
    fn into_patch(self) -> AuctionSchedulePatch {
        AuctionSchedulePatch {
            bidding_opens: clearable(self.bidding_opens),
            live_starts: clearable(self.live_starts),
            lots_begin_closing: clearable(self.lots_begin_closing),
            scheduled_end: clearable(self.scheduled_end),
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AuctionAdminData {
    auction_id: AuctionId,
    listing_source_id: ListingSourceId,
    source_auction_id: String,
    name: Option<LocalizedTextData>,
    description: Option<LocalizedTextData>,
    catalogue_url: Option<Url>,
    format: Option<&'static str>,
    schedule: AuctionScheduleResponseData,
    reported_status: Option<&'static str>,
    reported_lot_count: Option<u32>,
    expected_version: u64,

    #[serde(with = "time::serde::rfc3339")]
    created: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    updated: OffsetDateTime,
}

impl From<AuctionAdminDetailsView> for AuctionAdminData {
    fn from(value: AuctionAdminDetailsView) -> Self {
        Self {
            auction_id: value.auction_id,
            listing_source_id: value.key.listing_source_id(),
            source_auction_id: value.key.source_auction_id().to_string(),
            name: value.name.map(LocalizedTextData::from),
            description: value.description.map(LocalizedTextData::from),
            catalogue_url: value.catalogue_url,
            format: value.format.map(AuctionFormat::as_str),
            schedule: AuctionScheduleResponseData::from(value.schedule),
            reported_status: value.reported_status.map(AuctionReportedStatus::as_str),
            reported_lot_count: value
                .reported_lot_count
                .map(ReportedCatalogueLotCount::value),
            expected_version: value.version.into_inner(),

            created: value.created,
            updated: value.updated,
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AuctionScheduleResponseData {
    #[serde(with = "time::serde::rfc3339::option")]
    bidding_opens: Option<OffsetDateTime>,
    #[serde(with = "time::serde::rfc3339::option")]
    live_starts: Option<OffsetDateTime>,
    #[serde(with = "time::serde::rfc3339::option")]
    lots_begin_closing: Option<OffsetDateTime>,
    #[serde(with = "time::serde::rfc3339::option")]
    scheduled_end: Option<OffsetDateTime>,
}

impl From<AuctionSchedule> for AuctionScheduleResponseData {
    fn from(value: AuctionSchedule) -> Self {
        Self {
            bidding_opens: value.bidding_opens(),
            live_starts: value.live_starts(),
            lots_begin_closing: value.lots_begin_closing(),
            scheduled_end: value.scheduled_end(),
        }
    }
}

fn source_auction_id(value: String) -> Result<SourceAuctionId, ApiError> {
    SourceAuctionId::try_from(value).map_err(|_| {
        invalid_body("sourceAuctionId must be nonblank, NUL-free, and at most 512 UTF-8 bytes.")
    })
}

fn auction_name(
    value: LocalizedTextData,
) -> Result<localization::Localized<localization::Language, AuctionName>, ApiError> {
    AuctionName::try_from(value.text)
        .map(|payload| localization::Localized::new(value.language, payload))
        .map_err(|_| {
            invalid_body("name.text must be nonblank, NUL-free, and at most 512 UTF-8 bytes.")
        })
}

fn auction_description(
    value: LocalizedTextData,
) -> Result<localization::Localized<localization::Language, AuctionDescription>, ApiError> {
    AuctionDescription::try_from(value.text)
        .map(|payload| localization::Localized::new(value.language, payload))
        .map_err(|_| invalid_body("description.text must be valid nonblank sanitized text of at most 65536 UTF-8 bytes."))
}

fn auction_format(value: String) -> Result<AuctionFormat, ApiError> {
    value
        .parse()
        .map_err(|_| invalid_body("format must be LIVE or TIMED."))
}

fn auction_status(value: String) -> Result<AuctionReportedStatus, ApiError> {
    value.parse().map_err(|_| {
        invalid_body(
            "reportedStatus must be SCHEDULED, IN_PROGRESS, ENDED, POSTPONED, or CANCELLED.",
        )
    })
}

fn map_patch<T, U>(
    value: PatchValue<T>,
    map: impl Fn(T) -> Result<U, ApiError>,
) -> Result<PatchField<U>, ApiError> {
    match value {
        PatchValue::Omitted => Ok(PatchField::Unchanged),
        PatchValue::Null => Ok(PatchField::Clear),
        PatchValue::Value(value) => map(value).map(PatchField::Set),
    }
}

fn invalid_body(detail: &str) -> ApiError {
    ApiError::bad_request(BAD_BODY_VALUE).with_detail(detail)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_map_create_payload_with_rfc3339_schedule() -> Result<(), ApiError> {
        let listing_source_id = ListingSourceId::new();
        let payload = serde_json::json!({
            "listingSourceId": listing_source_id,
            "sourceAuctionId": " sale / 42 ",
            "format": "TIMED",
            "schedule": {
                "lotsBeginClosing": "2026-10-18T16:03:00Z"
            }
        });
        let command = CreateAuctionCommand::try_from(
            serde_json::from_value::<CreateAuctionData>(payload)
                .map_err(|_| invalid_body("invalid test payload"))?,
        )?;

        assert_eq!("sale / 42", command.source_auction_id.as_ref());
        assert_eq!(Some(AuctionFormat::Timed), command.format);
        assert_eq!(
            Some(time::macros::datetime!(2026-10-18 16:03 UTC)),
            command.schedule.lots_begin_closing()
        );
        Ok(())
    }

    #[test]
    fn should_serialize_schedule_as_direct_rfc3339_fields() -> Result<(), ApiError> {
        let schedule = AuctionSchedule::new(
            None,
            Some(time::macros::datetime!(2026-10-18 16:03 UTC)),
            None,
            None,
        )
        .map_err(|_| invalid_body("invalid test schedule"))?;

        let value = serde_json::to_value(AuctionScheduleResponseData::from(schedule))
            .map_err(|_| invalid_body("failed to serialize test schedule"))?;

        assert_eq!(
            Some(&serde_json::Value::String(
                "2026-10-18T16:03:00Z".to_owned()
            )),
            value.get("liveStarts")
        );
        assert!(value.get("precision").is_none());
        assert!(value.get("sourceTimezone").is_none());
        Ok(())
    }

    #[test]
    fn should_reject_noncanonical_auction_codes_and_unknown_members() {
        for body in [
            r#"{"listingSourceId":"ls_01jgfjjz4ne2g0000000000000","sourceAuctionId":"sale","format":"timed"}"#,
            r#"{"listingSourceId":"ls_01jgfjjz4ne2g0000000000000","sourceAuctionId":"sale","auctionId":"auc_01jgfjjz4ne2g0000000000000"}"#,
        ] {
            let result = serde_json::from_str::<CreateAuctionData>(body)
                .map_err(|_| invalid_body("invalid body"))
                .and_then(CreateAuctionCommand::try_from);
            assert!(result.is_err());
        }
    }

    #[test]
    fn should_distinguish_patch_omission_from_clear() -> Result<(), ApiError> {
        let auction_id = AuctionId::new();
        let omitted: UpdateAuctionData = serde_json::from_str(r#"{"expectedVersion":1}"#)
            .map_err(|_| invalid_body("invalid test payload"))?;
        let cleared: UpdateAuctionData =
            serde_json::from_str(r#"{"expectedVersion":1,"name":null}"#)
                .map_err(|_| invalid_body("invalid test payload"))?;

        assert!(matches!(
            omitted.into_command(auction_id)?.name,
            PatchField::Unchanged
        ));
        assert!(matches!(
            cleared.into_command(auction_id)?.name,
            PatchField::Clear
        ));
        Ok(())
    }
}

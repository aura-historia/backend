use application::error::{BoxError, box_error};
use auction_core::AuctionEventPayload;
use localization::{Language, Localized};
use serde_json::{Value, json};
use time::format_description::well_known::Rfc3339;

pub(crate) const AUCTION_EVENT_SCHEMA_VERSION: i16 = 1;

#[derive(Debug, thiserror::Error)]
pub(crate) enum AuctionEventCodecError {
    #[error("auction event time serialization failed")]
    Time(#[source] time::error::Format),
}

pub(crate) fn encode(payload: &AuctionEventPayload) -> Result<Value, AuctionEventCodecError> {
    match payload {
        AuctionEventPayload::Discovered(discovered) => Ok(json!({
            "listingSourceId": discovered.key().listing_source_id().as_uuid().to_string(),
            "sourceAuctionId": discovered.key().source_auction_id().as_ref(),
            "name": discovered.name().map(localized_name),
            "catalogueUrl": discovered.catalogue_url().map(url::Url::as_str),
            "format": discovered.format().map(|value| value.as_str()),
            "schedule": schedule(discovered.schedule())?,
            "reportedStatus": discovered.reported_status().map(|value| value.as_str()),
            "reportedLotCount": discovered.reported_lot_count().map(|value| value.value()),
        })),
        AuctionEventPayload::Changed(changed) => Ok(json!({
            "name": changed.name().map(|change| value_change(localized_name_option(change.previous()), localized_name_option(change.current()))),
            "catalogueUrl": changed.catalogue_url().map(|change| value_change(json!(change.previous().as_ref().map(url::Url::as_str)), json!(change.current().as_ref().map(url::Url::as_str)))),
            "format": changed.format().map(|change| value_change(json!(change.previous().map(|value| value.as_str())), json!(change.current().map(|value| value.as_str())))),
            "schedule": changed.schedule().map(|change| Ok::<_, AuctionEventCodecError>(value_change(schedule(change.previous())?, schedule(change.current())?))).transpose()?,
            "reportedStatus": changed.reported_status().map(|change| value_change(json!(change.previous().map(|value| value.as_str())), json!(change.current().map(|value| value.as_str())))),
            "reportedLotCount": changed.reported_lot_count().map(|change| value_change(json!(change.previous().map(|value| value.value())), json!(change.current().map(|value| value.value())))),
        })),
    }
}

fn value_change(previous: Value, current: Value) -> Value {
    json!({ "previous": previous, "current": current })
}

fn localized_name(value: &Localized<Language, auction_core::AuctionName>) -> Value {
    json!({"language": value.localization.as_str(), "text": value.payload.as_ref()})
}
fn localized_name_option(value: &Option<Localized<Language, auction_core::AuctionName>>) -> Value {
    value.as_ref().map(localized_name).unwrap_or(Value::Null)
}

fn schedule(schedule: &auction_core::AuctionSchedule) -> Result<Value, AuctionEventCodecError> {
    Ok(json!({
        "biddingOpens": schedule.bidding_opens().map(time).transpose()?,
        "liveStarts": schedule.live_starts().map(time).transpose()?,
        "lotsBeginClosing": schedule.lots_begin_closing().map(time).transpose()?,
        "scheduledEnd": schedule.scheduled_end().map(time).transpose()?,
    }))
}

fn time(value: time::OffsetDateTime) -> Result<Value, AuctionEventCodecError> {
    Ok(json!(
        value
            .format(&Rfc3339)
            .map_err(AuctionEventCodecError::Time)?
    ))
}

pub(crate) fn boxed(error: AuctionEventCodecError) -> BoxError {
    box_error(error)
}

#[cfg(test)]
mod tests {
    use super::{encode, time};
    use auction_core::{
        Auction, AuctionFormat, AuctionId, AuctionKey, AuctionSchedule, NewAuction, SourceAuctionId,
    };
    use listing_source_core::ListingSourceId;
    use serde_json::json;
    use time::macros::datetime;

    #[test]
    fn should_encode_schedule_time_as_direct_rfc3339_value() {
        let value = time(datetime!(2026-10-18 16:03 UTC))
            .unwrap_or_else(|error| panic!("valid schedule time: {error}"));

        assert_eq!(json!("2026-10-18T16:03:00Z"), value);
    }

    #[test]
    fn should_not_encode_auction_description_in_discovered_or_changed_events() {
        let mut auction = Auction::create(NewAuction {
            id: AuctionId::new(),
            key: AuctionKey::new(
                ListingSourceId::new(),
                SourceAuctionId::try_from("sale-42")
                    .unwrap_or_else(|error| panic!("valid source auction ID: {error}")),
            ),
            name: None,
            catalogue_url: None,
            format: None,
            schedule: AuctionSchedule::default(),
            reported_status: None,
            reported_lot_count: None,
        })
        .unwrap_or_else(|error| panic!("valid auction: {error}"));

        let discovered_payload = auction
            .take_pending_event_payload()
            .unwrap_or_else(|| panic!("discovery event payload"));
        let discovered = encode(&discovered_payload)
            .unwrap_or_else(|error| panic!("encoded discovery event: {error}"));
        assert!(discovered.get("description").is_none());

        let _ = auction.set_format(AuctionFormat::Timed);
        let changed_payload = auction
            .take_pending_event_payload()
            .unwrap_or_else(|| panic!("change event payload"));
        let changed = encode(&changed_payload)
            .unwrap_or_else(|error| panic!("encoded change event: {error}"));
        assert!(changed.get("description").is_none());
    }
}

use crate::use_cases::queries::get_auction::AuctionAdminDetailsView;
use application::{
    error::{BoxError, box_error, static_error},
    idempotency::{CommandOutcome, field, time_value, timestamp},
};
use auction_core::{
    AuctionKey, AuctionName, AuctionSchedule, ReportedCatalogueLotCount, SourceAuctionId,
};
use localization::{Language, Localized};
use serde_json::{Value, json};
use time::OffsetDateTime;

pub(crate) fn name_value(name: &Localized<Language, AuctionName>) -> Value {
    json!({"language":name.localization.as_str(),"text":name.payload.as_ref()})
}
pub(crate) fn instant(value: Option<OffsetDateTime>) -> Value {
    json!(value.map(|v| v.unix_timestamp_nanos().to_string()))
}
fn read_instant(v: &Value, key: &str) -> Result<Option<OffsetDateTime>, BoxError> {
    field::<Option<String>>(v, key)?
        .map(|v| {
            v.parse::<i128>()
                .map_err(box_error)
                .and_then(|v| OffsetDateTime::from_unix_timestamp_nanos(v).map_err(box_error))
        })
        .transpose()
}
pub(crate) fn schedule_value(s: &AuctionSchedule) -> Value {
    json!({"bidding_opens":instant(s.bidding_opens()),"live_starts":instant(s.live_starts()),"lots_begin_closing":instant(s.lots_begin_closing()),"scheduled_end":instant(s.scheduled_end())})
}
impl CommandOutcome for AuctionAdminDetailsView {
    fn encode(&self) -> Result<Value, BoxError> {
        Ok(
            json!({"auction_id":self.auction_id,"listing_source_id":self.key.listing_source_id(),"source_auction_id":self.key.source_auction_id().as_ref(),"name":self.name.as_ref().map(name_value),"catalogue_url":self.catalogue_url,"format":self.format.map(|v|v.as_str()),"schedule":schedule_value(&self.schedule),"reported_status":self.reported_status.map(|v|v.as_str()),"reported_lot_count":self.reported_lot_count.map(|v|v.value()),"version":self.version,"created":time_value(self.created)?,"updated":time_value(self.updated)?}),
        )
    }
    fn decode(v: Value) -> Result<Self, BoxError> {
        let name = field::<Option<Value>>(&v, "name")?
            .map(|name| {
                Ok::<_, BoxError>(Localized::new(
                    Language::from_code(&field::<String>(&name, "language")?)
                        .ok_or_else(|| static_error("invalid auction receipt language"))?,
                    AuctionName::try_from(field::<String>(&name, "text")?).map_err(box_error)?,
                ))
            })
            .transpose()?;
        let s = field::<Value>(&v, "schedule")?;
        Ok(Self {
            auction_id: field(&v, "auction_id")?,
            key: AuctionKey::new(
                field(&v, "listing_source_id")?,
                SourceAuctionId::try_from(field::<String>(&v, "source_auction_id")?)
                    .map_err(box_error)?,
            ),
            name,
            catalogue_url: field(&v, "catalogue_url")?,
            format: field::<Option<String>>(&v, "format")?
                .map(|v| v.parse())
                .transpose()
                .map_err(box_error)?,
            schedule: AuctionSchedule::new(
                read_instant(&s, "bidding_opens")?,
                read_instant(&s, "live_starts")?,
                read_instant(&s, "lots_begin_closing")?,
                read_instant(&s, "scheduled_end")?,
            )
            .map_err(box_error)?,
            reported_status: field::<Option<String>>(&v, "reported_status")?
                .map(|v| v.parse())
                .transpose()
                .map_err(box_error)?,
            reported_lot_count: field::<Option<u32>>(&v, "reported_lot_count")?
                .map(ReportedCatalogueLotCount::new),
            version: field(&v, "version")?,
            created: timestamp(&v, "created")?,
            updated: timestamp(&v, "updated")?,
        })
    }
}

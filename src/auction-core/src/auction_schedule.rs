use time::OffsetDateTime;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuctionSchedulePoint {
    BiddingOpens,
    LiveStarts,
    LotsBeginClosing,
    ScheduledEnd,
}

impl AuctionSchedulePoint {
    const fn label(self) -> &'static str {
        match self {
            Self::BiddingOpens => "bidding opens",
            Self::LiveStarts => "live starts",
            Self::LotsBeginClosing => "lots begin closing",
            Self::ScheduledEnd => "scheduled end",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("auction schedule {first} is after {second}", first = .first.label(), second = .second.label())]
pub struct InvalidAuctionSchedule {
    first: AuctionSchedulePoint,
    second: AuctionSchedulePoint,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AuctionSchedule {
    bidding_opens: Option<OffsetDateTime>,
    live_starts: Option<OffsetDateTime>,
    lots_begin_closing: Option<OffsetDateTime>,
    scheduled_end: Option<OffsetDateTime>,
}

impl AuctionSchedule {
    pub fn new(
        bidding_opens: Option<OffsetDateTime>,
        live_starts: Option<OffsetDateTime>,
        lots_begin_closing: Option<OffsetDateTime>,
        scheduled_end: Option<OffsetDateTime>,
    ) -> Result<Self, InvalidAuctionSchedule> {
        let schedule = Self {
            bidding_opens,
            live_starts,
            lots_begin_closing,
            scheduled_end,
        };
        schedule.validate()?;
        Ok(schedule)
    }

    pub const fn bidding_opens(&self) -> Option<OffsetDateTime> {
        self.bidding_opens
    }

    pub const fn live_starts(&self) -> Option<OffsetDateTime> {
        self.live_starts
    }

    pub const fn lots_begin_closing(&self) -> Option<OffsetDateTime> {
        self.lots_begin_closing
    }

    pub const fn scheduled_end(&self) -> Option<OffsetDateTime> {
        self.scheduled_end
    }

    fn validate(&self) -> Result<(), InvalidAuctionSchedule> {
        for (point, value) in [
            (AuctionSchedulePoint::BiddingOpens, self.bidding_opens),
            (AuctionSchedulePoint::LiveStarts, self.live_starts),
            (
                AuctionSchedulePoint::LotsBeginClosing,
                self.lots_begin_closing,
            ),
        ] {
            if let (Some(value), Some(end)) = (value, self.scheduled_end)
                && value > end
            {
                return Err(InvalidAuctionSchedule {
                    first: point,
                    second: AuctionSchedulePoint::ScheduledEnd,
                });
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{AuctionSchedule, AuctionSchedulePoint, InvalidAuctionSchedule};
    use time::macros::datetime;

    #[test]
    fn should_accept_an_empty_schedule() {
        assert_eq!(
            Ok(AuctionSchedule::default()),
            AuctionSchedule::new(None, None, None, None)
        );
    }

    #[test]
    fn should_reject_comparable_exact_milestone_after_scheduled_end() {
        let schedule = AuctionSchedule::new(
            Some(datetime!(2026-05-13 11:00 +02:00)),
            None,
            None,
            Some(datetime!(2026-05-13 10:00 +02:00)),
        );

        assert_eq!(
            Err(InvalidAuctionSchedule {
                first: AuctionSchedulePoint::BiddingOpens,
                second: AuctionSchedulePoint::ScheduledEnd,
            }),
            schedule
        );
    }

    #[test]
    fn should_compare_all_schedule_instants() {
        assert!(
            AuctionSchedule::new(
                Some(datetime!(2026-05-14 00:00 UTC)),
                None,
                None,
                Some(datetime!(2026-05-13 23:00 UTC)),
            )
            .is_err()
        );
    }
}

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
        if let (Some(bidding_opens), Some(live_starts)) = (self.bidding_opens, self.live_starts)
            && bidding_opens > live_starts
        {
            return Err(InvalidAuctionSchedule {
                first: AuctionSchedulePoint::BiddingOpens,
                second: AuctionSchedulePoint::LiveStarts,
            });
        }
        if let (Some(bidding_opens), Some(lots_begin_closing)) =
            (self.bidding_opens, self.lots_begin_closing)
            && bidding_opens > lots_begin_closing
        {
            return Err(InvalidAuctionSchedule {
                first: AuctionSchedulePoint::BiddingOpens,
                second: AuctionSchedulePoint::LotsBeginClosing,
            });
        }

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
    fn should_accept_bidding_open_before_live_start() {
        assert!(
            AuctionSchedule::new(
                Some(datetime!(2026-05-13 10:00 UTC)),
                Some(datetime!(2026-05-13 11:00 UTC)),
                None,
                None,
            )
            .is_ok()
        );
    }

    #[test]
    fn should_accept_bidding_open_equal_to_live_start() {
        assert!(
            AuctionSchedule::new(
                Some(datetime!(2026-05-13 10:00 UTC)),
                Some(datetime!(2026-05-13 10:00 UTC)),
                None,
                None,
            )
            .is_ok()
        );
    }

    #[test]
    fn should_reject_bidding_open_after_live_start() {
        assert_eq!(
            Err(InvalidAuctionSchedule {
                first: AuctionSchedulePoint::BiddingOpens,
                second: AuctionSchedulePoint::LiveStarts,
            }),
            AuctionSchedule::new(
                Some(datetime!(2026-05-13 11:00 UTC)),
                Some(datetime!(2026-05-13 10:00 UTC)),
                None,
                None,
            )
        );
    }

    #[test]
    fn should_accept_bidding_open_before_lots_begin_closing() {
        assert!(
            AuctionSchedule::new(
                Some(datetime!(2026-05-13 10:00 UTC)),
                None,
                Some(datetime!(2026-05-13 11:00 UTC)),
                None,
            )
            .is_ok()
        );
    }

    #[test]
    fn should_accept_bidding_open_equal_to_lots_begin_closing() {
        assert!(
            AuctionSchedule::new(
                Some(datetime!(2026-05-13 10:00 UTC)),
                None,
                Some(datetime!(2026-05-13 10:00 UTC)),
                None,
            )
            .is_ok()
        );
    }

    #[test]
    fn should_reject_bidding_open_after_lots_begin_closing() {
        assert_eq!(
            Err(InvalidAuctionSchedule {
                first: AuctionSchedulePoint::BiddingOpens,
                second: AuctionSchedulePoint::LotsBeginClosing,
            }),
            AuctionSchedule::new(
                Some(datetime!(2026-05-13 11:00 UTC)),
                None,
                Some(datetime!(2026-05-13 10:00 UTC)),
                None,
            )
        );
    }

    #[test]
    fn should_accept_schedule_with_all_milestones_at_or_before_end() {
        assert!(
            AuctionSchedule::new(
                Some(datetime!(2026-05-13 08:00 UTC)),
                Some(datetime!(2026-05-13 09:00 UTC)),
                Some(datetime!(2026-05-13 10:00 UTC)),
                Some(datetime!(2026-05-13 10:00 UTC)),
            )
            .is_ok()
        );
    }

    #[test]
    fn should_reject_bidding_open_after_scheduled_end() {
        assert_eq!(
            Err(InvalidAuctionSchedule {
                first: AuctionSchedulePoint::BiddingOpens,
                second: AuctionSchedulePoint::ScheduledEnd,
            }),
            AuctionSchedule::new(
                Some(datetime!(2026-05-13 11:00 UTC)),
                None,
                None,
                Some(datetime!(2026-05-13 10:00 UTC)),
            )
        );
    }

    #[test]
    fn should_reject_live_start_after_scheduled_end() {
        assert_eq!(
            Err(InvalidAuctionSchedule {
                first: AuctionSchedulePoint::LiveStarts,
                second: AuctionSchedulePoint::ScheduledEnd,
            }),
            AuctionSchedule::new(
                None,
                Some(datetime!(2026-05-13 11:00 UTC)),
                None,
                Some(datetime!(2026-05-13 10:00 UTC)),
            )
        );
    }

    #[test]
    fn should_reject_lots_begin_closing_after_scheduled_end() {
        assert_eq!(
            Err(InvalidAuctionSchedule {
                first: AuctionSchedulePoint::LotsBeginClosing,
                second: AuctionSchedulePoint::ScheduledEnd,
            }),
            AuctionSchedule::new(
                None,
                None,
                Some(datetime!(2026-05-13 11:00 UTC)),
                Some(datetime!(2026-05-13 10:00 UTC)),
            )
        );
    }

    #[test]
    fn should_keep_sparse_schedules_valid() {
        for schedule in [
            (Some(datetime!(2026-05-13 10:00 UTC)), None, None, None),
            (None, Some(datetime!(2026-05-13 10:00 UTC)), None, None),
            (None, None, Some(datetime!(2026-05-13 10:00 UTC)), None),
            (None, None, None, Some(datetime!(2026-05-13 10:00 UTC))),
        ] {
            assert!(AuctionSchedule::new(schedule.0, schedule.1, schedule.2, schedule.3).is_ok());
        }
    }
}

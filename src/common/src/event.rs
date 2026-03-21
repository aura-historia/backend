use crate::event_id::EventId;
use time::OffsetDateTime;

#[derive(Debug, Clone, PartialEq)]
pub struct Event<AggregateId, Payload> {
    pub aggregate_id: AggregateId,
    pub event_id: EventId,
    pub timestamp: OffsetDateTime,
    pub payload: Payload,
}

impl<AggregateId, Payload> Event<AggregateId, Payload> {
    pub fn map_payload<R, F>(self, f: F) -> Event<AggregateId, R>
    where
        F: FnMut(Payload) -> R,
    {
        let mut f = f;
        Event {
            aggregate_id: self.aggregate_id,
            event_id: self.event_id,
            timestamp: self.timestamp,
            payload: f(self.payload),
        }
    }
}

#[cfg(feature = "test-data")]
mod faker {
    use super::*;
    use fake::{Dummy, Fake, Faker, RngExt};

    impl<AggregateId: Dummy<Faker>, Payload: Dummy<Faker>> Dummy<Faker>
        for Event<AggregateId, Payload>
    {
        fn dummy_with_rng<R: RngExt + ?Sized>(config: &Faker, rng: &mut R) -> Self {
            Event {
                aggregate_id: config.fake_with_rng(rng),
                event_id: config.fake_with_rng(rng),
                timestamp: OffsetDateTime::now_utc(),
                payload: config.fake_with_rng(rng),
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use crate::event::Event;
        use fake::{Fake, Faker};
        use uuid::Uuid;

        #[test]
        fn should_fake_event() {
            let _ = Faker.fake::<Event<Uuid, String>>();
        }
    }
}

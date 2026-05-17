pub trait Aggregate: Sized {
    type Event;
    type Error;

    fn replay(events: impl IntoIterator<Item = Self::Event>) -> Result<Self, Self::Error>;
    fn apply_event(&mut self, event: Self::Event) -> Result<(), Self::Error>;
}

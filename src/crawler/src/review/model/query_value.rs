//! The named values a console list accepts in a query parameter, such as a review status or a sort
//! column. A value the table does not name is refused with the names it does, so a request never
//! falls back to another reading.

/// A closed set of values named in a query parameter.
pub trait QueryValue: Sized + Copy + 'static {
    /// Every accepted name with its value.
    const VALUES: &'static [(&'static str, Self)];

    /// The value `name` names, or `None` when it names none.
    fn parse(name: &str) -> Option<Self> {
        Self::VALUES
            .iter()
            .find_map(|(accepted, value)| (*accepted == name).then_some(*value))
    }

    /// The accepted names, for a refusal.
    fn accepted() -> String {
        Self::VALUES
            .iter()
            .map(|(name, _)| *name)
            .collect::<Vec<_>>()
            .join(", ")
    }
}

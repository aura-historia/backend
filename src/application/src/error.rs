use std::error::Error;

pub type BoxError = Box<dyn Error + Send + Sync + 'static>;

#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub struct StaticError(pub &'static str);

pub fn box_error(source: impl Error + Send + Sync + 'static) -> BoxError {
    Box::new(source)
}

pub fn static_error(message: &'static str) -> BoxError {
    Box::new(StaticError(message))
}

/// How a use case failed, in bounded labels that every boundary recording the error shares.
///
/// It names the kind of collaborator, not the technology behind it: the composition root that
/// wired the port knows which system that is. No label carries error text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailureKind {
    /// The authoritative store failed; only the cause chain can classify it further.
    Storage,
    /// The authoritative store holds state that violates its invariants.
    InvalidPersistedState,
    /// An outbound provider failed, with this bounded lower-case class.
    Provider(&'static str),
    /// The use case refused the request itself, with this bounded lower-case class.
    Rejected(&'static str),
}

/// The error followed by its `source` chain, outermost first.
pub fn error_chain<'a>(
    error: &'a (dyn Error + 'static),
) -> impl Iterator<Item = &'a (dyn Error + 'static)> {
    std::iter::successors(Some(error), |error| (*error).source())
}

/// The first error of type `T` in the chain, starting with `error` itself.
pub fn find_source<'a, T: Error + 'static>(error: &'a (dyn Error + 'static)) -> Option<&'a T> {
    error_chain(error).find_map(|error| error.downcast_ref::<T>())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, thiserror::Error)]
    #[error("outer")]
    struct Outer(#[source] BoxError);

    #[test]
    fn should_find_the_first_source_of_a_type_below_wrapping_errors() {
        let error = Outer(box_error(Outer(static_error("cause"))));

        let found = find_source::<StaticError>(&error);

        assert_eq!(Some("cause"), found.map(|error| error.0));
        assert_eq!(3, error_chain(&error).count());
    }

    #[test]
    fn should_match_the_error_itself_before_its_sources() {
        let error = StaticError("itself");

        assert_eq!(
            Some("itself"),
            find_source::<StaticError>(&error).map(|error| error.0)
        );
        assert!(find_source::<Outer>(&error).is_none());
    }
}

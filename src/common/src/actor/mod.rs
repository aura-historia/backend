use crate::actor::domain::Actor;

pub mod data;
#[cfg(feature = "opensearch")]
pub mod document;
pub mod domain;
pub mod record;

#[derive(Copy, Clone, Eq, PartialEq, Debug, Hash)]
#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
pub struct RequestContext {
    pub actor: Actor,
}

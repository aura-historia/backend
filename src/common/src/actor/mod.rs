use crate::actor::domain::Actor;

pub mod data;
#[cfg(feature = "opensearch")]
pub mod document;
pub mod domain;
pub mod record;

#[derive(Copy, Clone, Eq, PartialEq, Debug, Hash)]
pub struct RequestContext {
    pub actor: Actor,
}

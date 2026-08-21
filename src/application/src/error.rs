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

pub mod mail_id;
pub mod payload;
pub mod template;

#[cfg(feature = "queue")]
pub mod queue_service;

#[cfg(feature = "send")]
pub mod record;
#[cfg(feature = "send")]
pub mod repository;
#[cfg(feature = "send")]
pub mod send_service;

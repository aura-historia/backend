pub mod mail_id;
pub mod payload;
pub mod template;

#[cfg(feature = "send")]
pub mod record;
#[cfg(feature = "send")]
pub mod s3_adapter;
#[cfg(feature = "send")]
pub mod ses_adapter;

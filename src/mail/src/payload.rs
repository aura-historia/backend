use crate::template::MailTemplate;
use serde::{Deserialize, Serialize};
use serde_email::Email;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MailPayload {
    pub sender: Email,
    pub recipient: Email,
    pub subject: String,
    pub template: MailTemplate,
    pub data: serde_json::Value,
}

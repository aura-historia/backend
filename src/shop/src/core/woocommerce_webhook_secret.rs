use common::string_newtype;
use serde::{Deserialize, Serialize};

string_newtype!(WoocommerceWebhookSecret, derives(Serialize, Deserialize));

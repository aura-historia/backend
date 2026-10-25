//! Complete production consumer catalogue over local HTTP, Lambda and the official SDK client.
use test_api::{Postgres, aura_integration_test};
const SCHEMA: Postgres = Postgres::new("migrations");
#[path = "consumer_release/catalogue.rs"]
mod catalogue;
#[path = "consumer_release/removed_partner_tools.rs"]
mod removed_partner_tools;
#[path = "consumer_release/support.rs"]
mod support;

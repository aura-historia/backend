//! Complete production catalogue and partner workflow over local HTTP with current business state.
use test_api::{Postgres, aura_integration_test};
const SCHEMA: Postgres = Postgres::new("migrations");
#[path = "partner_release/authority.rs"]
mod authority;
#[path = "partner_release/catalogue.rs"]
mod catalogue;
#[path = "partner_release/support.rs"]
mod support;
#[path = "partner_release/uncertainty.rs"]
mod uncertainty;
#[path = "partner_release/workflow.rs"]
mod workflow;

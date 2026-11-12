//! Operator presentation joined to ListingSource reads.
//!
//! Readers join the operator Party and its `ACTIVE` Partnership in the statement that already
//! loads the ListingSource, so partner status is read from authoritative state without an extra
//! round trip and is never copied into projections.

use party_core::party_name::{PartyName, PartyNameError};
use product_listing_service::ports::ListingSourceOperatorSummary;

pub(crate) fn listing_source_operator(
    name: String,
    partner: bool,
) -> Result<ListingSourceOperatorSummary, PartyNameError> {
    Ok(ListingSourceOperatorSummary {
        name: PartyName::try_from(name)?,
        partner,
    })
}

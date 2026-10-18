//! Explicitly accept a country snapshot from an authorized private destination.
use application::operation_context::OperationContext;
use product_listing_core::shipping_search::ShippingSearch;
use user_core::user_address::{BrowsingDestination, UserAddressId};
use user_service::address_book::{AddressBookError, AddressBookQueries};

#[derive(Clone, PartialEq, Eq)]
pub enum AcceptShippingDestination {
    Address(UserAddressId),
    Preference,
}
impl std::fmt::Debug for AcceptShippingDestination {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("AcceptShippingDestination(<redacted>)")
    }
}

pub(crate) async fn accept(
    queries: Option<&dyn AddressBookQueries>,
    context: &OperationContext,
    selection: Option<&AcceptShippingDestination>,
    shipping: Option<&ShippingSearch>,
) -> Result<(), AddressBookError> {
    let Some(selection) = selection else {
        return Ok(());
    };
    let query = shipping
        .filter(|q| q.country().is_some())
        .ok_or(AddressBookError::InvalidInput)?;
    let queries = queries.ok_or(AddressBookError::InvalidInput)?;
    let explicit = match selection {
        AcceptShippingDestination::Address(id) => Some(BrowsingDestination::Address(*id)),
        AcceptShippingDestination::Preference => None,
    };
    // This existing use case checks ownership, suspension, delegated users:read and current
    // evidence validity. Only the submitted country is accepted; provider facts are not saved.
    let facts = queries.resolve_destination_facts(context, explicit).await?;
    let submitted = facts.submitted.ok_or(AddressBookError::InvalidInput)?;
    if submitted.reference_release() != query.reference_release()
        || submitted
            .country()
            .or_else(|| submitted.subdivision().map(|s| s.country()))
            != query.country()
    {
        return Err(AddressBookError::InvalidInput);
    }
    Ok(())
}

use crate::product_lifecycle::ProductLifecycle;

#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Debug, Clone, PartialEq)]
pub enum ProductListingLifecycleEventPayload {
    Deleted(ProductListingDeletedLifecycleEventPayload),
}

impl ProductListingLifecycleEventPayload {
    pub fn event_type(&self) -> &'static str {
        match self {
            ProductListingLifecycleEventPayload::Deleted(_) => "LIFECYCLE_DELETED",
        }
    }
}

#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Debug, Clone, PartialEq)]
pub struct ProductListingDeletedLifecycleEventPayload {
    pub old_lifecycle: ProductLifecycle,
    pub new_lifecycle: ProductLifecycle,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_return_deleted_event_type() {
        let event = ProductListingLifecycleEventPayload::Deleted(
            ProductListingDeletedLifecycleEventPayload {
                old_lifecycle: ProductLifecycle::Active,
                new_lifecycle: ProductLifecycle::Deleted,
            },
        );

        assert_eq!("LIFECYCLE_DELETED", event.event_type());
    }
}

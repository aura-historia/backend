use crate::product_lifecycle::ProductLifecycle;

#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Debug, Clone, PartialEq)]
pub enum ProductLifecycleEventPayload {
    Deleted(ProductDeletedLifecycleEventPayload),
}

impl ProductLifecycleEventPayload {
    pub fn event_type(&self) -> &'static str {
        match self {
            ProductLifecycleEventPayload::Deleted(_) => "LIFECYCLE_DELETED",
        }
    }
}

#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Debug, Clone, PartialEq)]
pub struct ProductDeletedLifecycleEventPayload {
    pub old_lifecycle: ProductLifecycle,
    pub new_lifecycle: ProductLifecycle,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_return_deleted_event_type() {
        let event = ProductLifecycleEventPayload::Deleted(ProductDeletedLifecycleEventPayload {
            old_lifecycle: ProductLifecycle::Active,
            new_lifecycle: ProductLifecycle::Deleted,
        });

        assert_eq!("LIFECYCLE_DELETED", event.event_type());
    }
}

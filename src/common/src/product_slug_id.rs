// Legacy shim. Owner: product-core. Remove after legacy Product consumers migrate.
pub use product_core::product_slug_id::ProductSlugId;

impl From<crate::slug_id::SlugId<6>> for ProductSlugId {
    fn from(value: crate::slug_id::SlugId<6>) -> Self {
        Self::from(value.to_string())
    }
}

impl From<ProductSlugId> for crate::slug_id::SlugId<6> {
    fn from(value: ProductSlugId) -> Self {
        Self::from(value.to_string())
    }
}

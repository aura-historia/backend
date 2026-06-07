crate::slug_id_newtype!(SellerSlugId, 0);

impl From<crate::shop_slug_id::ShopSlugId> for SellerSlugId {
    fn from(value: crate::shop_slug_id::ShopSlugId) -> Self {
        let value: crate::slug_id::SlugId<0> = value.into();
        Self::from(value)
    }
}

impl From<SellerSlugId> for crate::shop_slug_id::ShopSlugId {
    fn from(value: SellerSlugId) -> Self {
        let value: crate::slug_id::SlugId<0> = value.into();
        Self::from(value)
    }
}

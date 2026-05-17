use crate::core::product::Product;
use crate::dynamodb::product_record::{mk_gsi2_pk, mk_gsi2_sk, mk_pk};
use common::has_key::HasKey;
use common::product_id::{ProductId, ProductKey};
use common::shop_id::ShopId;
use common::shops_product_id::ShopsProductId;
use common::slug_id::SlugId;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProductMetaRecord {
    pub pk: String,
    pub sk: String,
    pub gsi2_pk: String,
    pub gsi2_sk: String,
    pub product_id: ProductId,
    pub product_slug_id: SlugId<6>,
    pub shop_slug_id: SlugId<0>,
    pub seller_slug_id: SlugId<0>,
    pub shop_id: ShopId,
    pub shops_product_id: ShopsProductId,
    pub event_version: u64,
}

pub fn mk_sk() -> &'static str {
    "product#meta"
}

impl ProductMetaRecord {
    pub fn from_product(product: &Product, event_version: u64) -> Self {
        Self {
            pk: mk_pk(&product.shop_id, &product.shops_product_id),
            sk: mk_sk().to_owned(),
            gsi2_pk: mk_gsi2_pk(&product.shop_slug_id, &product.product_slug_id),
            gsi2_sk: mk_gsi2_sk().to_owned(),
            product_id: product.product_id,
            product_slug_id: product.product_slug_id.clone(),
            shop_slug_id: product.shop_slug_id.clone(),
            seller_slug_id: product.seller_slug_id.clone(),
            shop_id: product.shop_id,
            shops_product_id: product.shops_product_id.clone(),
            event_version,
        }
    }
}

impl HasKey for ProductMetaRecord {
    type Key = ProductKey;

    fn key(&self) -> Self::Key {
        ProductKey {
            shop_id: self.shop_id,
            shops_product_id: self.shops_product_id.clone(),
        }
    }
}

#[cfg(feature = "test-data")]
mod faker {
    use super::*;
    use fake::{Dummy, Fake, Faker, RngExt};

    impl Dummy<Faker> for ProductMetaRecord {
        fn dummy_with_rng<R: RngExt + ?Sized>(config: &Faker, rng: &mut R) -> Self {
            let product: Product = config.fake_with_rng(rng);
            Self::from_product(&product, config.fake_with_rng(rng))
        }
    }
}

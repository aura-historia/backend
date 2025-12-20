use common::product_id::ProductId;

pub trait HasProductId {
    fn product_id(&self) -> ProductId;
}

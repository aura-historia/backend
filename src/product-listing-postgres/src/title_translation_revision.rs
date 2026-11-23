//! What a listing's title translations do when its content revision advances
//! (docs/product-listing.md#title-translation). Runs inside the listing update's transaction.

use product_listing_core::product_listing_id::ProductListingId;
use sqlx::PgConnection;

/// Drops every translation of the listing; the new revision's translation job replaces them.
pub(crate) async fn discard_title_translations(
    connection: &mut PgConnection,
    product_listing_id: ProductListingId,
) -> Result<(), sqlx::Error> {
    sqlx::query("DELETE FROM product_listing_translations WHERE product_listing_id = $1")
        .bind(product_listing_id.as_uuid())
        .execute(connection)
        .await?;
    Ok(())
}

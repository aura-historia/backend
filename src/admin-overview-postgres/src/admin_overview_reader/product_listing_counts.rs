use super::{AdminOverviewRowMappingError, count};
use admin_overview_service::use_cases::get_admin_overview::{
    AdminOverviewActiveListingAvailabilityCounts, AdminOverviewProductListingLifecycleCounts,
    AdminOverviewProductListings,
};

#[derive(Debug, sqlx::FromRow)]
pub(super) struct ProductListingCountsRow {
    product_listings_total: i64,
    product_listings_active: i64,
    product_listings_withdrawn: i64,
    active_available: i64,
    active_in_stock: i64,
    active_limited_availability: i64,
    active_back_order: i64,
    active_made_to_order: i64,
    active_pre_order: i64,
    active_pre_sale: i64,
    active_unavailable: i64,
    active_reserved: i64,
    active_out_of_stock: i64,
    active_sold_out: i64,
    active_without_availability: i64,
}

impl TryFrom<ProductListingCountsRow> for AdminOverviewProductListings {
    type Error = AdminOverviewRowMappingError;

    fn try_from(row: ProductListingCountsRow) -> Result<Self, Self::Error> {
        Ok(Self {
            total: count("product_listings_total", row.product_listings_total)?,
            by_lifecycle: AdminOverviewProductListingLifecycleCounts {
                active: count("product_listings_active", row.product_listings_active)?,
                withdrawn: count("product_listings_withdrawn", row.product_listings_withdrawn)?,
            },
            active_availability: AdminOverviewActiveListingAvailabilityCounts {
                available: count("active_available", row.active_available)?,
                in_stock: count("active_in_stock", row.active_in_stock)?,
                limited_availability: count(
                    "active_limited_availability",
                    row.active_limited_availability,
                )?,
                back_order: count("active_back_order", row.active_back_order)?,
                made_to_order: count("active_made_to_order", row.active_made_to_order)?,
                pre_order: count("active_pre_order", row.active_pre_order)?,
                pre_sale: count("active_pre_sale", row.active_pre_sale)?,
                unavailable: count("active_unavailable", row.active_unavailable)?,
                reserved: count("active_reserved", row.active_reserved)?,
                out_of_stock: count("active_out_of_stock", row.active_out_of_stock)?,
                sold_out: count("active_sold_out", row.active_sold_out)?,
            },
            active_without_availability: count(
                "active_without_availability",
                row.active_without_availability,
            )?,
        })
    }
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;

    pub(in crate::admin_overview_reader) fn zero_counts_row() -> ProductListingCountsRow {
        ProductListingCountsRow {
            product_listings_total: 0,
            product_listings_active: 0,
            product_listings_withdrawn: 0,
            active_available: 0,
            active_in_stock: 0,
            active_limited_availability: 0,
            active_back_order: 0,
            active_made_to_order: 0,
            active_pre_order: 0,
            active_pre_sale: 0,
            active_unavailable: 0,
            active_reserved: 0,
            active_out_of_stock: 0,
            active_sold_out: 0,
            active_without_availability: 0,
        }
    }
}

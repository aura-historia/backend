use application::error::box_error;
use product_listing_service::ports::{
    ProductListingRepositoryError, admin_product_listing_reader::AdminProductListingReadError,
};

pub(crate) fn is_temporary(error: &sqlx::Error) -> bool {
    match error {
        sqlx::Error::Io(_)
        | sqlx::Error::Tls(_)
        | sqlx::Error::PoolTimedOut
        | sqlx::Error::PoolClosed
        | sqlx::Error::WorkerCrashed => true,
        sqlx::Error::Database(error) => error.code().is_some_and(|code| {
            code.starts_with("08")
                || code.starts_with("53")
                || matches!(
                    code.as_ref(),
                    "40001" | "40P01" | "55P03" | "57014" | "57P01" | "57P02" | "57P03"
                )
        }),
        _ => false,
    }
}

pub(crate) fn repository_query_error(error: sqlx::Error) -> ProductListingRepositoryError {
    if is_temporary(&error) {
        ProductListingRepositoryError::TemporarilyUnavailable {
            source: box_error(error),
        }
    } else {
        ProductListingRepositoryError::QueryFailed {
            source: box_error(error),
        }
    }
}

pub(crate) fn admin_read_error(error: sqlx::Error) -> AdminProductListingReadError {
    if is_temporary(&error) {
        AdminProductListingReadError::TemporarilyUnavailable {
            source: box_error(error),
        }
    } else {
        AdminProductListingReadError::QueryFailed {
            source: box_error(error),
        }
    }
}

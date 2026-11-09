//! Parsing of the query parameters the console's paginated reads share. A value that is given but
//! not accepted is refused with what is accepted, so a request that cannot be read as asked is
//! answered with `400` instead of a different page.

use crate::review::model::PageSize;
use std::collections::HashMap;
use std::str::FromStr;

/// The boolean parameter `name`: `true` or `false`, or `None` when it is not given.
pub(super) fn optional_bool(
    query: &HashMap<String, String>,
    name: &str,
) -> Result<Option<bool>, String> {
    match query.get(name).map(String::as_str) {
        None => Ok(None),
        Some("true") => Ok(Some(true)),
        Some("false") => Ok(Some(false)),
        Some(_) => Err(format!("{name} must be true or false")),
    }
}

/// The `after` parameter as the id of type `T`, the `next_after` of the previous page, or `None`
/// when it is not given. `what` names the id in the refusal, such as "a review id".
pub(super) fn after_cursor<T: FromStr>(
    query: &HashMap<String, String>,
    what: &str,
) -> Result<Option<T>, String> {
    query
        .get("after")
        .map(|after| {
            after.parse::<T>().map_err(|_| {
                format!("after must be {what}, such as the next_after of the previous page")
            })
        })
        .transpose()
}

/// The `limit` parameter as the page size, the default when it is not given.
pub(super) fn page_size(query: &HashMap<String, String>) -> Result<PageSize, String> {
    match query.get("limit") {
        None => Ok(PageSize::DEFAULT),
        Some(limit) => limit.parse().ok().and_then(PageSize::new).ok_or_else(|| {
            format!(
                "limit must be a whole number from 1 to {}",
                PageSize::MAX.get()
            )
        }),
    }
}

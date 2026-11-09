//! Parsing of the query parameters the console's paginated reads share. A value that is given but
//! not accepted is refused with what is accepted, so a request that cannot be read as asked is
//! answered with `400` instead of a different page.

use crate::review::model::{
    ListWindow, MAX_SEARCH_TERM_CHARS, PageSize, QueryValue, SearchTerm, SortDirection,
};
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

/// The window `limit` (at most [`ListWindow::MAX_LIMIT`], 100 by default) and `offset` (0 by
/// default) ask for.
pub(super) fn list_window(query: &HashMap<String, String>) -> Result<ListWindow, String> {
    let limit = match query.get("limit") {
        None => ListWindow::DEFAULT_LIMIT,
        Some(limit) => limit.parse().map_err(|_| limit_refusal())?,
    };
    let offset = match query.get("offset") {
        None => 0,
        Some(offset) => offset
            .parse()
            .map_err(|_| "offset must be a whole number".to_owned())?,
    };
    ListWindow::new(limit, offset).ok_or_else(limit_refusal)
}

fn limit_refusal() -> String {
    format!(
        "limit must be a whole number from 1 to {}",
        ListWindow::MAX_LIMIT
    )
}

/// Refuses a parameter of `query` that is not one of `accepted`, so a misspelt filter is not
/// taken for no filter.
pub(super) fn reject_unknown(
    query: &HashMap<String, String>,
    accepted: &[&str],
) -> Result<(), String> {
    match query.keys().find(|name| !accepted.contains(&name.as_str())) {
        None => Ok(()),
        Some(name) => Err(format!(
            "unknown parameter {name}; accepted: {}",
            accepted.join(", ")
        )),
    }
}

/// The parameter `name` as the value of `T` it names, or `None` when it is not given.
pub(super) fn named<T: QueryValue>(
    query: &HashMap<String, String>,
    name: &str,
) -> Result<Option<T>, String> {
    query
        .get(name)
        .map(|value| {
            T::parse(value).ok_or_else(|| format!("{name} must be one of {}", T::accepted()))
        })
        .transpose()
}

/// The parameter `name` as the id of type `T`, or `None` when it is not given. `what` names the
/// id in the refusal, such as "a ListingSource id".
pub(super) fn id_param<T: FromStr>(
    query: &HashMap<String, String>,
    name: &str,
    what: &str,
) -> Result<Option<T>, String> {
    query
        .get(name)
        .map(|value| {
            value
                .parse::<T>()
                .map_err(|_| format!("{name} must be {what}"))
        })
        .transpose()
}

/// The search text `q`, or `None` when it is not given or blank.
pub(super) fn search_term(query: &HashMap<String, String>) -> Result<Option<SearchTerm>, String> {
    match query.get("q") {
        Some(text) if !text.trim().is_empty() => SearchTerm::new(text).map(Some).ok_or_else(|| {
            format!(
                "q must be at most {MAX_SEARCH_TERM_CHARS} characters without control characters"
            )
        }),
        _ => Ok(None),
    }
}

/// The sort direction `dir`, or `default` when it is not given.
pub(super) fn sort_direction(
    query: &HashMap<String, String>,
    default: SortDirection,
) -> Result<SortDirection, String> {
    Ok(named::<SortDirection>(query, "dir")?.unwrap_or(default))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn query_of(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
            .collect()
    }

    #[test]
    fn should_refuse_a_parameter_that_is_not_accepted() {
        let query = query_of(&[("humen_required", "true")]);

        let error = reject_unknown(&query, &["human_required", "limit"]).unwrap_err();

        assert_eq!(
            error,
            "unknown parameter humen_required; accepted: human_required, limit"
        );
        assert_eq!(reject_unknown(&query_of(&[]), &["limit"]), Ok(()));
    }

    #[test]
    fn should_name_what_a_named_parameter_accepts() {
        assert_eq!(
            named::<SortDirection>(&query_of(&[("dir", "up")]), "dir"),
            Err("dir must be one of asc, desc".to_owned())
        );
        assert_eq!(
            named::<SortDirection>(&query_of(&[("dir", "asc")]), "dir"),
            Ok(Some(SortDirection::Ascending))
        );
        assert_eq!(named::<SortDirection>(&query_of(&[]), "dir"), Ok(None));
    }

    #[test]
    fn should_take_a_blank_search_for_no_search_and_refuse_one_that_is_too_long() {
        assert_eq!(search_term(&query_of(&[("q", "  ")])), Ok(None));
        assert_eq!(search_term(&query_of(&[])), Ok(None));
        assert_eq!(
            search_term(&query_of(&[("q", " Antik ")]))
                .map(|term| term.map(|t| t.as_str().to_owned())),
            Ok(Some("Antik".to_owned()))
        );
        assert!(search_term(&query_of(&[("q", &"a".repeat(101))])).is_err());
    }

    #[test]
    fn should_read_the_window_and_refuse_one_beyond_the_largest() {
        let window = |pairs: &[(&str, &str)]| list_window(&query_of(pairs));

        assert_eq!(window(&[]), Ok(ListWindow::default()));
        assert_eq!(
            window(&[("limit", "25"), ("offset", "50")]),
            Ok(ListWindow::new(25, 50).unwrap_or_default())
        );
        for refused in [
            [("limit", "0")],
            [("limit", "201")],
            [("limit", "many")],
            [("offset", "-1")],
        ] {
            assert!(window(&refused).is_err(), "{refused:?}");
        }
    }
}

//! The console's activity reads: `GET /api/review-events`, the events of all reviews since a time,
//! which the review agents' daily digest and the Overview's recent-activity feed read, and `GET
//! /api/source-events`, what the crawler did to ListingSources. The contract is in
//! `docs/crawler/operations.md` (review service).

use super::query_params::{named, reject_unknown};
use super::{ReviewServer, read_error};
use crate::review::http::{HttpResponse, ParsedRequest};
use crate::review::model::SortDirection;
use serde_json::json;
use std::collections::HashMap;
use time::OffsetDateTime;

/// The most events one answer holds; a client reads on from the last event's time, as the review
/// agents' daily digest does.
const MAX_LISTED_EVENTS: usize = 1_000;

/// The window a feed request asks for.
#[derive(Debug, PartialEq, Eq)]
struct EventWindow {
    since: OffsetDateTime,
    limit: usize,
    direction: SortDirection,
}

impl ReviewServer {
    /// `GET /api/review-events?since=<RFC 3339 time>`: the events of all reviews since then without
    /// their details, and whether more were left out. Oldest first unless `dir=desc`.
    pub(super) async fn review_events_since(&self, request: &ParsedRequest<'_>) -> HttpResponse {
        let window = match event_window(&request.query, SortDirection::Ascending) {
            Ok(window) => window,
            Err(error) => return HttpResponse::json(400, &json!({ "error": error })),
        };
        match self
            .repository
            .list_review_events_since(window.since, window.limit, window.direction)
            .await
        {
            Ok((events, truncated)) => {
                HttpResponse::json(200, &json!({ "events": events, "truncated": truncated }))
            }
            Err(error) => read_error(error),
        }
    }

    /// `GET /api/source-events?since=<RFC 3339 time>`: what the crawler did to ListingSources since
    /// then, newest first, and whether more were left out.
    pub(super) async fn source_events_since(&self, request: &ParsedRequest<'_>) -> HttpResponse {
        let window = match event_window(&request.query, SortDirection::Descending) {
            Ok(window) => window,
            Err(error) => return HttpResponse::json(400, &json!({ "error": error })),
        };
        match self
            .source_events
            .list_source_events(window.since, window.limit)
            .await
        {
            Ok((events, truncated)) => {
                HttpResponse::json(200, &json!({ "events": events, "truncated": truncated }))
            }
            Err(error) => read_error(error),
        }
    }
}

/// The window `query` asks for: `since`, an RFC 3339 time, `limit`, at most
/// [`MAX_LISTED_EVENTS`], and `dir`, `default_direction` when it is not given.
fn event_window(
    query: &HashMap<String, String>,
    default_direction: SortDirection,
) -> Result<EventWindow, String> {
    reject_unknown(query, &["since", "limit", "dir"])?;
    let since = query
        .get("since")
        .and_then(|since| {
            OffsetDateTime::parse(since, &time::format_description::well_known::Rfc3339).ok()
        })
        .ok_or_else(|| "since must be an RFC 3339 time, such as 2026-10-28T06:00:00Z".to_owned())?;
    let limit = match query.get("limit") {
        None => MAX_LISTED_EVENTS,
        Some(limit) => limit
            .parse::<usize>()
            .ok()
            .filter(|limit| (1..=MAX_LISTED_EVENTS).contains(limit))
            .ok_or_else(|| format!("limit must be a whole number from 1 to {MAX_LISTED_EVENTS}"))?,
    };
    Ok(EventWindow {
        since,
        limit,
        direction: named::<SortDirection>(query, "dir")?.unwrap_or(default_direction),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn window_of(pairs: &[(&str, &str)], default: SortDirection) -> Result<EventWindow, String> {
        event_window(
            &pairs
                .iter()
                .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
                .collect(),
            default,
        )
    }

    #[test]
    fn should_read_the_window_asked_for_and_default_the_rest() {
        let window = window_of(
            &[("since", "2026-11-01T06:00:00Z")],
            SortDirection::Ascending,
        )
        .unwrap_or_else(|error| panic!("{error}"));

        assert_eq!(window.limit, MAX_LISTED_EVENTS);
        assert_eq!(window.direction, SortDirection::Ascending);
        let newest = window_of(
            &[
                ("since", "2026-11-01T06:00:00Z"),
                ("dir", "desc"),
                ("limit", "50"),
            ],
            SortDirection::Ascending,
        )
        .unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(
            (newest.limit, newest.direction),
            (50, SortDirection::Descending)
        );
    }

    #[test]
    fn should_refuse_a_window_that_cannot_be_read_as_asked() {
        for pairs in [
            vec![],
            vec![("since", "yesterday")],
            vec![("since", "2026-11-01T06:00:00Z"), ("limit", "0")],
            vec![("since", "2026-11-01T06:00:00Z"), ("limit", "1001")],
            vec![("since", "2026-11-01T06:00:00Z"), ("dir", "up")],
            vec![("since", "2026-11-01T06:00:00Z"), ("after", "x")],
        ] {
            assert!(
                window_of(&pairs, SortDirection::Ascending).is_err(),
                "{pairs:?}"
            );
        }
    }
}

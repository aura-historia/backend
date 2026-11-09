//! What a read that failed on an unreadable stored row says about the row. A read fails closed on
//! invalid persisted state, so one such row fails the list or feed that reaches it; naming the row
//! lets the console tell the operator which stored object to fix instead of a bare server error.

use super::review_queue::UnreadableAutoSchemaEvaluation;
use crate::review::repository::PersistedObjectIdError;

/// The stored object a failed read could not decode.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PersistedRowFailure {
    /// What the row is, such as `review`, or `row` when the failure names no object type.
    pub object: &'static str,
    /// The identifier of the row as it is stored, when the failure names one.
    pub id: Option<String>,
    /// What was wrong with it.
    pub detail: String,
}

/// The failure of `error` as an unreadable stored row, or `None` when `error` is no decode
/// failure, such as a lost connection.
pub fn persisted_row_failure(error: &sqlx::Error) -> Option<PersistedRowFailure> {
    let sqlx::Error::Decode(source) = error else {
        return None;
    };
    if let Some(invalid) = source.downcast_ref::<PersistedObjectIdError>() {
        return Some(PersistedRowFailure {
            object: object_of(invalid.object_type),
            id: Some(invalid.value.to_string()),
            detail: invalid.to_string(),
        });
    }
    if let Some(unreadable) = source.downcast_ref::<UnreadableAutoSchemaEvaluation>() {
        return Some(PersistedRowFailure {
            object: "review",
            id: Some(unreadable.review_id.to_string()),
            detail: unreadable.to_string(),
        });
    }
    Some(PersistedRowFailure {
        object: "row",
        id: None,
        detail: source.to_string(),
    })
}

fn object_of(object_type: &str) -> &'static str {
    match object_type {
        "CrawlerReviewId" => "review",
        "CrawlerReviewPageId" => "review page",
        "CrawlerReviewUrlId" => "review URL",
        "CrawlerReviewEventId" => "review event",
        "ListingSourceId" => "ListingSource",
        _ => "row",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_name_the_object_of_an_invalid_stored_id() {
        let value = uuid::Uuid::new_v4();
        let error = crate::review::repository::crawler_review_id_from_uuid(value)
            .expect_err("a v4 uuid is no review id");

        let failure = persisted_row_failure(&error).expect("a decode failure");

        assert_eq!(failure.object, "review");
        assert_eq!(failure.id, Some(value.to_string()));
    }

    #[test]
    fn should_describe_any_other_decode_failure_as_a_row_and_no_other_error_at_all() {
        let decode = sqlx::Error::Decode("invalid persisted actor `x`".into());

        let failure = persisted_row_failure(&decode).expect("a decode failure");

        assert_eq!((failure.object, failure.id), ("row", None));
        assert!(persisted_row_failure(&sqlx::Error::RowNotFound).is_none());
    }
}

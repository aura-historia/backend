crate::uuid_v7_newtype!(EventId);

#[cfg(feature = "api")]
pub mod api {
    use crate::{
        api::{
            error::ApiError,
            error_code::{BAD_PAGE_SIZE_VALUE, BAD_PATH_PARAMETER_VALUE, INVALID_UUID},
        },
        event_id::EventId,
        pagination::cursor::Cursor,
    };
    use aws_lambda_events::query_map::QueryMap;
    use std::collections::HashMap;

    pub fn extract_event_id_path(
        path_params: &HashMap<String, String>,
    ) -> Result<EventId, ApiError> {
        path_params
            .get("eventId")
            .map(|s| EventId::try_from(s.as_str()))
            .transpose()
            .map_err(|err| {
                let msg = err.to_string();
                ApiError::bad_request(INVALID_UUID, Box::new(err))
                    .with_path_field("eventId")
                    .with_detail(msg)
            })?
            .ok_or(
                ApiError::bad_request(
                    BAD_PATH_PARAMETER_VALUE,
                    "Missing path parameter 'eventId'.".into(),
                )
                .with_path_field("eventId")
                .with_detail("Missing field 'eventId'."),
            )
    }

    pub fn extract_event_id_cursor_query(
        query: &QueryMap,
    ) -> Result<Option<Cursor<EventId>>, ApiError> {
        let search_after = query
            .first("searchAfter")
            .map(str::trim)
            .map(EventId::try_from)
            .transpose()
            .map_err(|err| {
                let msg = err.to_string();
                ApiError::bad_request(INVALID_UUID, Box::new(err))
                    .with_query_field("searchAfter")
                    .with_detail(msg)
            })?;
        let size = query
            .first("size")
            .map(str::trim)
            .map(|size| size.parse::<u64>())
            .transpose()
            .map_err(|err| {
                let msg = err.to_string();
                ApiError::bad_request(BAD_PAGE_SIZE_VALUE, Box::new(err))
                    .with_query_field("size")
                    .with_detail(msg)
            })?
            .map(|size| size.min(100));

        if let Some(size) = size {
            Ok(Some(Cursor { search_after, size }))
        } else {
            Ok(None)
        }
    }
}

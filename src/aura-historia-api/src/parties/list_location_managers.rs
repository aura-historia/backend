use crate::{
    auth::protected_context,
    error::{ApiError, BAD_QUERY_PARAMETER_VALUE},
    state::PartyLocationsState,
    wire::{parse_path_object_id, parse_query_object_id},
};
use application::pagination::Cursor;
use axum::{
    Json,
    extract::{Path, RawQuery, State},
    http::HeaderMap,
    response::{IntoResponse, Response},
};
use partnership_core::partnership_id::PartnershipId;
use party_core::party_id::PartyId;
use party_service::use_cases::queries::list_party_location_managers::*;
use serde::{Deserialize, Serialize};
use serde_email::Email;
use user_core::{first_name::FirstName, last_name::LastName, user_id::UserId};

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ListQuery {
    user_id: Option<String>,
    access: Option<String>,
    size: Option<String>,
    search_after: Option<String>,
}

fn bad_query(field: &'static str, detail: impl std::fmt::Display) -> ApiError {
    ApiError::bad_request(BAD_QUERY_PARAMETER_VALUE)
        .with_query_field(field)
        .with_detail(detail.to_string())
}

fn parse_query(
    party_id: PartyId,
    raw: Option<&str>,
) -> Result<ListPartyLocationManagersRequest, ApiError> {
    let query: ListQuery = serde_qs::Config::new()
        .use_form_encoding(true)
        .deserialize_str(raw.unwrap_or_default())
        .map_err(|error| {
            ApiError::bad_request(BAD_QUERY_PARAMETER_VALUE).with_detail(error.to_string())
        })?;
    let access = query
        .access
        .as_deref()
        .map(|value| match value {
            "EXPLICIT" => Ok(PartyLocationManagerAccessKind::Explicit),
            "PARTNERSHIP" => Ok(PartyLocationManagerAccessKind::Partnership),
            _ => Err(bad_query("access", "Expected EXPLICIT or PARTNERSHIP.")),
        })
        .transpose()?;
    Ok(ListPartyLocationManagersRequest {
        party_id,
        user_id: query
            .user_id
            .as_deref()
            .map(|value| parse_query_object_id(value, "userId", "User"))
            .transpose()?,
        access,
        cursor: Cursor {
            size: query
                .size
                .as_deref()
                .map(|value| {
                    value
                        .parse::<u64>()
                        .map(|size| size.clamp(1, 100))
                        .map_err(|error| bad_query("size", error))
                })
                .transpose()?
                .unwrap_or(21),
            search_after: query
                .search_after
                .as_deref()
                .map(|value| parse_query_object_id(value, "searchAfter", "User"))
                .transpose()?,
        },
    })
}

#[derive(Serialize)]
#[serde(tag = "type", rename_all = "SCREAMING_SNAKE_CASE")]
enum AccessData {
    Explicit,
    Partnership {
        #[serde(rename = "partnershipIds")]
        partnership_ids: Vec<PartnershipId>,
    },
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ManagerData {
    user_id: UserId,
    email: Email,
    #[serde(skip_serializing_if = "Option::is_none")]
    first_name: Option<FirstName>,
    #[serde(skip_serializing_if = "Option::is_none")]
    last_name: Option<LastName>,
    access: Vec<AccessData>,
}

impl From<PartyLocationManagerView> for ManagerData {
    fn from(view: PartyLocationManagerView) -> Self {
        Self {
            user_id: view.user_id,
            email: view.email,
            first_name: view.first_name,
            last_name: view.last_name,
            access: view
                .access
                .into_iter()
                .map(|reason| match reason {
                    PartyLocationManagerAccess::Explicit => AccessData::Explicit,
                    PartyLocationManagerAccess::Partnership { partnership_ids } => {
                        AccessData::Partnership { partnership_ids }
                    }
                })
                .collect(),
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct CollectionData {
    items: Vec<ManagerData>,
    size: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    search_after: Option<UserId>,
}

pub async fn list(
    State(state): State<PartyLocationsState>,
    Path(party): Path<String>,
    headers: HeaderMap,
    RawQuery(raw): RawQuery,
) -> Response {
    let (context, _) = match protected_context(state.authenticator.as_ref(), &headers).await {
        Ok(value) => value,
        Err(response) => return *response,
    };
    let request = match parse_path_object_id(&party, "party_id", "Party")
        .and_then(|party_id| parse_query(party_id, raw.as_deref()))
    {
        Ok(request) => request,
        Err(error) => return error.into_response(),
    };
    match state.list_managers.execute(&context, request).await {
        Ok(page) => Json(CollectionData {
            size: page.items.len(),
            items: page.items.into_iter().map(Into::into).collect(),
            search_after: page.cursor.search_after,
        })
        .into_response(),
        Err(error) => ApiError::from(error).into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_filters_and_strict_user_cursors_with_bounded_sizes() {
        let party_id = PartyId::new();
        let user_id = UserId::new();
        let after = UserId::new();
        let parsed = parse_query(
            party_id,
            Some(&format!(
                "userId={user_id}&access=PARTNERSHIP&size=200&searchAfter={after}"
            )),
        )
        .unwrap();
        assert_eq!(Some(user_id), parsed.user_id);
        assert_eq!(
            Some(PartyLocationManagerAccessKind::Partnership),
            parsed.access
        );
        assert_eq!(Some(after), parsed.cursor.search_after);
        assert_eq!(100, parsed.cursor.size);
        assert_eq!(21, parse_query(party_id, None).unwrap().cursor.size);
        assert_eq!(
            1,
            parse_query(party_id, Some("size=0")).unwrap().cursor.size
        );
        for query in [
            "userId=bad".to_owned(),
            format!("userId={party_id}"),
            format!("userId={}", user_id.as_uuid()),
            "access=explicit".to_owned(),
            "access=".to_owned(),
            "access=OTHER".to_owned(),
            "size=nope".to_owned(),
            "size=-1".to_owned(),
            "size=18446744073709551616".to_owned(),
            "searchAfter=bad".to_owned(),
            format!("searchAfter={party_id}"),
            "unexpected=true".to_owned(),
        ] {
            assert!(parse_query(party_id, Some(&query)).is_err(), "{query}");
        }
    }

    #[test]
    fn collection_uses_strict_user_cursor_and_omits_it_on_empty_terminal_page() {
        let user_id = UserId::new();
        let value = serde_json::to_value(CollectionData {
            items: vec![],
            size: 0,
            search_after: Some(user_id),
        })
        .unwrap();
        assert_eq!(serde_json::json!(user_id), value["searchAfter"]);
        let value = serde_json::to_value(CollectionData {
            items: vec![],
            size: 0,
            search_after: None,
        })
        .unwrap();
        assert_eq!(serde_json::json!({"items":[], "size":0}), value);
    }
}

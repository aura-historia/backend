use application::pagination::CursoredResult;
use serde::Serialize;
use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct JsonCursoredData<T> {
    pub(crate) items: Vec<T>,
    pub(crate) size: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) search_after: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) total: Option<u64>,
}

impl<T, TData> From<CursoredResult<T, Value>> for JsonCursoredData<TData>
where
    T: Into<TData>,
{
    fn from(result: CursoredResult<T, Value>) -> Self {
        let items = result.items.into_iter().map(Into::into).collect::<Vec<_>>();
        Self::new(items, result.cursor.search_after, result.total)
    }
}

impl<T> JsonCursoredData<T> {
    pub(crate) fn new(items: Vec<T>, search_after: Option<Value>, total: Option<u64>) -> Self {
        Self {
            size: items.len() as u64,
            items,
            search_after,
            total,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use application::pagination::Cursor;
    use serde_json::json;

    #[test]
    fn size_reports_returned_items_instead_of_the_requested_page_size() {
        let empty: JsonCursoredData<Value> = CursoredResult {
            items: Vec::<Value>::new(),
            cursor: Cursor {
                size: 21,
                search_after: None,
            },
            total: None,
        }
        .into();
        assert_eq!(0, empty.size);

        let one: JsonCursoredData<Value> = CursoredResult {
            items: vec![json!({ "id": "one" })],
            cursor: Cursor {
                size: 21,
                search_after: None,
            },
            total: None,
        }
        .into();
        assert_eq!(1, one.size);
    }
}

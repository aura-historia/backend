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
        Self {
            items: result.items.into_iter().map(Into::into).collect(),
            size: result.cursor.size,
            search_after: result.cursor.search_after,
            total: result.total,
        }
    }
}

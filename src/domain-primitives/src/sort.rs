#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SortOrder {
    Asc,
    Desc,
}

impl SortOrder {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Asc => "asc",
            Self::Desc => "desc",
        }
    }
}

impl From<SortOrder> for &'static str {
    fn from(value: SortOrder) -> Self {
        value.as_str()
    }
}

impl TryFrom<&str> for SortOrder {
    type Error = String;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        match value {
            "asc" => Ok(Self::Asc),
            "desc" => Ok(Self::Desc),
            invalid => Err(format!("Expected any of: 'asc', 'desc'. Got: '{invalid}'")),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Sort<T> {
    pub sort: T,
    pub order: SortOrder,
}

impl<T> Sort<T> {
    pub fn map<U, F>(self, f: F) -> Sort<U>
    where
        F: FnOnce(T) -> U,
    {
        Sort {
            sort: f(self.sort),
            order: self.order,
        }
    }
}

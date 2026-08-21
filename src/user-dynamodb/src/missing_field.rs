#[derive(Debug, Clone, Copy)]
pub struct MissingPersistenceField(&'static str);

impl MissingPersistenceField {
    pub(crate) fn new(field: &'static str) -> Self {
        Self(field)
    }
}

impl std::fmt::Display for MissingPersistenceField {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.0)
    }
}

impl std::error::Error for MissingPersistenceField {}

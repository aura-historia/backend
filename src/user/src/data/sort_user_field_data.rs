use crate::core::sort_user_field::SortUserField;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SortUserFieldData {
    #[default]
    Score,
    Email,
    FirstName,
    LastName,
    Tier,
    Role,
    Updated,
    Created,
}

impl SortUserFieldData {
    pub fn as_str(&self) -> &'static str {
        match self {
            SortUserFieldData::Score => "score",
            SortUserFieldData::Email => "email",
            SortUserFieldData::FirstName => "firstName",
            SortUserFieldData::LastName => "lastName",
            SortUserFieldData::Tier => "tier",
            SortUserFieldData::Role => "role",
            SortUserFieldData::Updated => "updated",
            SortUserFieldData::Created => "created",
        }
    }
}

impl From<SortUserFieldData> for &'static str {
    fn from(value: SortUserFieldData) -> Self {
        value.as_str()
    }
}

impl<'a> TryFrom<&'a str> for SortUserFieldData {
    type Error = String;

    fn try_from(value: &'a str) -> Result<Self, Self::Error> {
        match value {
            "score" => Ok(SortUserFieldData::Score),
            "email" => Ok(SortUserFieldData::Email),
            "firstName" => Ok(SortUserFieldData::FirstName),
            "lastName" => Ok(SortUserFieldData::LastName),
            "tier" => Ok(SortUserFieldData::Tier),
            "role" => Ok(SortUserFieldData::Role),
            "updated" => Ok(SortUserFieldData::Updated),
            "created" => Ok(SortUserFieldData::Created),
            invalid => Err(format!(
                "Expected any of: 'score', 'email', 'firstName', 'lastName', 'tier', 'role', 'updated', 'created'. Got: '{invalid}'"
            )),
        }
    }
}

impl From<SortUserFieldData> for SortUserField {
    fn from(value: SortUserFieldData) -> Self {
        match value {
            SortUserFieldData::Score => SortUserField::Score,
            SortUserFieldData::Email => SortUserField::Email,
            SortUserFieldData::FirstName => SortUserField::FirstName,
            SortUserFieldData::LastName => SortUserField::LastName,
            SortUserFieldData::Tier => SortUserField::Tier,
            SortUserFieldData::Role => SortUserField::Role,
            SortUserFieldData::Updated => SortUserField::Updated,
            SortUserFieldData::Created => SortUserField::Created,
        }
    }
}

use crate::{slug_id::SlugId, string_newtype};

pub type CategoryId = SlugId<0>;

string_newtype!(CategoryKey, derives(serde::Serialize, serde::Deserialize));

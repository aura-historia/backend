use crate::{slug_id::SlugId, string_newtype};

pub type CategorySlugId = SlugId<0>;

string_newtype!(CategoryKey, serde);

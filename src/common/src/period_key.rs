use crate::{slug_id::SlugId, string_newtype};

pub type PeriodId = SlugId<0>;

string_newtype!(PeriodKey, serde);

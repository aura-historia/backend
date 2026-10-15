//! Advertised delivery assertions and deterministic assessment, with no I/O or clock.
//! Adapters decode through constructors; services supply current dependencies explicitly.

macro_rules! codes {
    ($ty:ident, $($variant:ident => $code:literal),+ $(,)?) => {
        impl $ty {
            pub fn as_str(self) -> &'static str { match self { $(Self::$variant => $code),+ } }
            pub fn from_code(value: &str) -> Option<Self> { match value { $($code => Some(Self::$variant)),+, _ => None } }
        }
    };
}

pub mod assessment;
pub mod coverage;
pub mod inputs;
pub mod pricing;
pub mod profile;
pub mod selection;
pub mod terms;
pub mod time;

#[cfg(any(test, feature = "test-data"))]
pub mod fixtures;

#[cfg(test)]
mod tests;

/// Constructor failures carry no source text, addresses or buyer data.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum InvalidShippingValue {
    #[error("shipping text must be nonblank, single-line and within its byte limit")]
    Text,
    #[error("shipping range bounds are invalid")]
    Bounds,
    #[error("monetary bounds must use the same currency")]
    CurrencyMismatch,
    #[error("shipping values do not belong to the owning listing source")]
    SourceMismatch,
    #[error("shipping rules use incompatible geographic reference releases")]
    ReferenceReleaseMismatch,
    #[error("shipping rule collection is empty, oversized or contains duplicate keys")]
    RuleCollection,
    #[error("postal rule form is unsupported or malformed for its country")]
    PostalRule,
    #[error("shipping site must contain a geographic or position assertion")]
    EmptySite,
    #[error("a source default can select only an active shipping profile")]
    RetiredProfile,
}

pub(crate) fn validate_text(value: &str, max_bytes: usize) -> Result<(), InvalidShippingValue> {
    if value.trim().is_empty()
        || value.len() > max_bytes
        || value
            .chars()
            .any(|ch| ch.is_control() || matches!(ch, '\u{2028}' | '\u{2029}'))
    {
        return Err(InvalidShippingValue::Text);
    }
    Ok(())
}

pub mod discovery;

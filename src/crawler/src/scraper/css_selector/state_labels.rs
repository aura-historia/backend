//! A source's mapping from call-to-action labels to availabilities.
//!
//! Dealer templates show the state as a shop-specific label, such as "Secure this piece",
//! "ITEM HAS BEEN SOLD" or "Add to enquiry". The label mapping lives with the schema that
//! extracts the label, so it goes through review with the selectors and a label the mapping
//! does not know shows in the schema matrix.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// The availability a label stands for, named after the schema.org availability values the
/// normalization worker resolves.
#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum LabelAvailability {
    InStock,
    LimitedAvailability,
    BackOrder,
    MadeToOrder,
    PreOrder,
    PreSale,
    Reserved,
    OutOfStock,
    SoldOut,
}

impl LabelAvailability {
    /// The raw `state` the worker resolves: the schema.org availability URL.
    pub(crate) fn raw_state(self) -> &'static str {
        match self {
            Self::InStock => "https://schema.org/InStock",
            Self::LimitedAvailability => "https://schema.org/LimitedAvailability",
            Self::BackOrder => "https://schema.org/BackOrder",
            Self::MadeToOrder => "https://schema.org/MadeToOrder",
            Self::PreOrder => "https://schema.org/PreOrder",
            Self::PreSale => "https://schema.org/PreSale",
            Self::Reserved => "https://schema.org/Reserved",
            Self::OutOfStock => "https://schema.org/OutOfStock",
            Self::SoldOut => "https://schema.org/SoldOut",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum StateLabelsError {
    #[error("a state label is blank")]
    BlankLabel,
    #[error("state labels `{first}` and `{second}` differ only in case or whitespace")]
    DuplicateLabel { first: String, second: String },
}

/// Labels compared after whitespace collapse, trimming and case folding, so the mapping
/// does not depend on a template's capitalisation or markup whitespace.
fn normalize(label: &str) -> String {
    label
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

/// The labels a source's state rule can capture, with the availability each stands for.
/// A stored map is checked on load: a blank label, or two labels that compare equal, is
/// refused, so one label never has two meanings.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(
    try_from = "BTreeMap<String, LabelAvailability>",
    into = "BTreeMap<String, LabelAvailability>"
)]
#[schemars(
    description = "Maps the call-to-action labels of this source's state rule to availabilities, compared ignoring case and whitespace. Keys are labels as the page shows them; values are IN_STOCK, LIMITED_AVAILABILITY, BACK_ORDER, MADE_TO_ORDER, PRE_ORDER, PRE_SALE, RESERVED, OUT_OF_STOCK or SOLD_OUT."
)]
pub struct StateLabels(BTreeMap<String, LabelAvailability>);

impl TryFrom<BTreeMap<String, LabelAvailability>> for StateLabels {
    type Error = StateLabelsError;

    fn try_from(labels: BTreeMap<String, LabelAvailability>) -> Result<Self, Self::Error> {
        let mut seen: BTreeMap<String, &str> = BTreeMap::new();
        for label in labels.keys() {
            let normalized = normalize(label);
            if normalized.is_empty() {
                return Err(StateLabelsError::BlankLabel);
            }
            if let Some(first) = seen.insert(normalized, label) {
                return Err(StateLabelsError::DuplicateLabel {
                    first: first.to_owned(),
                    second: label.clone(),
                });
            }
        }
        Ok(Self(labels))
    }
}

impl From<StateLabels> for BTreeMap<String, LabelAvailability> {
    fn from(labels: StateLabels) -> Self {
        labels.0
    }
}

impl StateLabels {
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// The availability `label` stands for, if the mapping knows it.
    pub fn resolve(&self, label: &str) -> Option<LabelAvailability> {
        let wanted = normalize(label);
        self.0
            .iter()
            .find(|(known, _)| normalize(known) == wanted)
            .map(|(_, availability)| *availability)
    }

    /// `state` as the raw state a schema with this mapping captures: the availability of a
    /// mapped label, any other value as extracted.
    pub(crate) fn map_state(&self, state: String) -> String {
        match self.resolve(&state) {
            Some(availability) => availability.raw_state().to_owned(),
            None => state,
        }
    }

    /// The label of `state` when it is a visible label the mapping does not know. A blank
    /// value is the blank check's finding, and a schema.org availability needs no mapping.
    pub(crate) fn unmapped_label(&self, state: &str) -> Option<String> {
        let label = state.trim();
        (!label.is_empty() && !label.contains("schema.org/") && self.resolve(label).is_none())
            .then(|| label.to_owned())
    }
}

/// Generated schemas carry no mapping: random labels would be blank or equivalent ones.
#[cfg(feature = "test-data")]
impl<T> fake::Dummy<T> for StateLabels {
    fn dummy_with_rng<R: fake::rand::Rng + ?Sized>(_: &T, _: &mut R) -> Self {
        Self::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn labels(entries: &[(&str, LabelAvailability)]) -> Result<StateLabels, StateLabelsError> {
        StateLabels::try_from(
            entries
                .iter()
                .map(|(label, availability)| ((*label).to_owned(), *availability))
                .collect::<BTreeMap<_, _>>(),
        )
    }

    #[test]
    fn should_resolve_labels_ignoring_case_and_whitespace() {
        let map = labels(&[("Secure this piece", LabelAvailability::InStock)]).unwrap();
        assert_eq!(
            map.resolve("  SECURE\n this   piece "),
            Some(LabelAvailability::InStock)
        );
        assert_eq!(map.resolve("Enquire"), None);
    }

    #[test]
    fn should_map_a_known_label_and_keep_an_unknown_value() {
        let map = labels(&[("ITEM HAS BEEN SOLD", LabelAvailability::SoldOut)]).unwrap();
        assert_eq!(
            map.map_state("item has been sold".to_owned()),
            "https://schema.org/SoldOut"
        );
        assert_eq!(map.map_state("ENQUIRE".to_owned()), "ENQUIRE");
    }

    #[test]
    fn should_report_only_visible_unknown_labels_as_unmapped() {
        let map = labels(&[("Sold", LabelAvailability::SoldOut)]).unwrap();
        assert_eq!(map.unmapped_label(" Enquire "), Some("Enquire".to_owned()));
        assert_eq!(map.unmapped_label("sold"), None);
        assert_eq!(map.unmapped_label("  "), None);
        assert_eq!(map.unmapped_label("https://schema.org/InStock"), None);
    }

    #[test]
    fn should_refuse_blank_and_equivalent_labels_on_load() {
        let blank = serde_json::from_str::<StateLabels>(r#"{" ": "SOLD_OUT"}"#);
        assert!(blank.is_err());
        let duplicate =
            serde_json::from_str::<StateLabels>(r#"{"Sold": "SOLD_OUT", "SOLD ": "IN_STOCK"}"#);
        assert!(duplicate.is_err());
        let unknown = serde_json::from_str::<StateLabels>(r#"{"Sold": "GONE"}"#);
        assert!(unknown.is_err());
    }
}

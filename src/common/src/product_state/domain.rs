use strum_macros::EnumCount;

use crate::language::domain::Language;

#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Copy, Clone, Eq, PartialEq, Debug, Hash, EnumCount)]
pub enum ProductState {
    Listed,
    Available,
    Reserved,
    Sold,
    Removed,
    Unknown,
}

impl ProductState {
    pub fn format_human_readable(&self, language: &Language) -> &'static str {
        match self {
            ProductState::Listed => match language {
                Language::De => "Gelistet",
                Language::En => "Listed",
                Language::Fr => "Listé",
                Language::Es => "Listado",
            },
            ProductState::Available => match language {
                Language::De => "Verfügbar",
                Language::En => "Available",
                Language::Fr => "Disponible",
                Language::Es => "Disponible",
            },
            ProductState::Reserved => match language {
                Language::De => "Reserviert",
                Language::En => "Reserved",
                Language::Fr => "Réservé",
                Language::Es => "Reservado",
            },
            ProductState::Sold => match language {
                Language::De => "Verkauft",
                Language::En => "Sold",
                Language::Fr => "Vendu",
                Language::Es => "Vendido",
            },
            ProductState::Removed => match language {
                Language::De => "Gelöscht",
                Language::En => "Removed",
                Language::Fr => "Supprimé",
                Language::Es => "Eliminado",
            },
            ProductState::Unknown => match language {
                Language::De => "Unbekannt",
                Language::En => "Unknown",
                Language::Fr => "Inconnu",
                Language::Es => "Desconocido",
            },
        }
    }
}

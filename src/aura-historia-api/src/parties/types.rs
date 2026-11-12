use partnership_core::partnership_id::PartnershipId;
use party_core::party_id::PartyId;
use party_service::use_cases::queries::{
    get_party::PartyDetailsView,
    search_parties::{PartySummary, SearchPartiesResult},
};
use serde::Serialize;
use time::OffsetDateTime;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PartyCollectionData {
    pub(crate) items: Vec<PartySummaryData>,
    pub(crate) size: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) search_after: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) total: Option<u64>,
}

impl From<SearchPartiesResult> for PartyCollectionData {
    fn from(result: SearchPartiesResult) -> Self {
        let items = result
            .items
            .into_iter()
            .map(PartySummaryData::from)
            .collect::<Vec<_>>();
        Self {
            size: items.len() as u64,
            items,
            search_after: result
                .cursor
                .search_after
                .map(super::search_parties::encode_search_after),
            total: result.total,
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PartyData {
    pub(crate) party_id: PartyId,
    pub(crate) party_slug_id: String,
    pub(crate) name: String,
    pub(crate) contact: PartyContactData,
    pub(crate) partner: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) partnership_id: Option<PartnershipId>,
    #[serde(with = "time::serde::rfc3339")]
    pub(crate) created: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub(crate) updated: OffsetDateTime,
}

impl From<PartyDetailsView> for PartyData {
    fn from(value: PartyDetailsView) -> Self {
        Self {
            party_id: value.party_id,
            party_slug_id: value.party_slug_id.to_string(),
            name: value.name.to_string(),
            contact: PartyContactData {
                phone: value.contact.phone,
                email: value.contact.email.map(|email| email.to_string()),
            },
            partner: value.partnership_id.is_some(),
            partnership_id: value.partnership_id,
            created: value.created,
            updated: value.updated,
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PartySummaryData {
    pub(crate) party_id: PartyId,
    pub(crate) party_slug_id: String,
    pub(crate) name: String,
    pub(crate) contact: PartyContactData,
    pub(crate) partner: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) partnership_id: Option<PartnershipId>,
    #[serde(with = "time::serde::rfc3339")]
    pub(crate) created: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub(crate) updated: OffsetDateTime,
}

impl From<PartySummary> for PartySummaryData {
    fn from(value: PartySummary) -> Self {
        Self {
            party_id: value.party_id,
            party_slug_id: value.party_slug_id.to_string(),
            name: value.name.to_string(),
            contact: PartyContactData {
                phone: value.contact.phone,
                email: value.contact.email.map(|email| email.to_string()),
            },
            partner: value.partnership_id.is_some(),
            partnership_id: value.partnership_id,
            created: value.created,
            updated: value.updated,
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PartyContactData {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) phone: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) email: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use party_core::party::{NewParty, Party, PartyContact};
    use party_core::party_name::PartyName;
    use party_core::party_slug_id::PartySlugId;
    use party_service::ports::{PartyStorageVersion, StoredParty};

    #[test]
    fn should_serialize_partner_status_and_active_partnership()
    -> Result<(), Box<dyn std::error::Error>> {
        let stored = StoredParty {
            party: Party::create(
                NewParty {
                    id: PartyId::new(),
                    name: PartyName::try_from("Ada Antiques GmbH")?,
                    contact: PartyContact::default(),
                },
                PartySlugId::raw("ada-antiques-gmbh")?,
            ),
            version: PartyStorageVersion::INITIAL,
            created: OffsetDateTime::UNIX_EPOCH,
            updated: OffsetDateTime::UNIX_EPOCH,
        };
        let partnership_id = PartnershipId::new();

        let partner = serde_json::to_value(PartyData::from(PartyDetailsView::new(
            stored.clone(),
            Some(partnership_id),
        )))?;
        let other = serde_json::to_value(PartyData::from(PartyDetailsView::new(stored, None)))?;

        assert_eq!(serde_json::json!(true), partner["partner"]);
        assert_eq!(serde_json::json!(partnership_id), partner["partnershipId"]);
        assert_eq!(serde_json::json!(false), other["partner"]);
        assert!(other.get("partnershipId").is_none());
        Ok(())
    }
}

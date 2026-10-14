use crate::use_cases::queries::get_party::PartyDetailsView;
use application::{
    error::{BoxError, box_error},
    idempotency::{CommandOutcome, field, time_value, timestamp},
};
use party_core::{party::PartyContact, party_name::PartyName, party_slug_id::PartySlugId};
use serde_json::{Value, json};

impl CommandOutcome for PartyDetailsView {
    fn encode(&self) -> Result<Value, BoxError> {
        Ok(
            json!({"party_id":self.party_id,"slug":self.party_slug_id.as_ref(),"name":self.name.as_ref(),"phone":self.contact.phone,"email":self.contact.email,"created":time_value(self.created)?,"updated":time_value(self.updated)?}),
        )
    }
    fn decode(v: Value) -> Result<Self, BoxError> {
        Ok(Self {
            party_id: field(&v, "party_id")?,
            party_slug_id: PartySlugId::raw(field::<String>(&v, "slug")?).map_err(box_error)?,
            name: PartyName::try_from(field::<String>(&v, "name")?).map_err(box_error)?,
            contact: PartyContact {
                phone: field(&v, "phone")?,
                email: field(&v, "email")?,
            },
            created: timestamp(&v, "created")?,
            updated: timestamp(&v, "updated")?,
        })
    }
}

impl application::idempotency::CommandOutcome
    for crate::use_cases::party_locations::PartyLocationView
{
    fn encode(&self) -> Result<serde_json::Value, BoxError> {
        Ok(
            serde_json::json!({"id":self.id,"party_id":self.party_id,"label":self.label.as_str(),"roles":self.roles.iter().map(|v|v.as_str()).collect::<Vec<_>>(),"geography":self.geography.as_ref().map(geo::data::geography_data::GeographicDescriptionData::from),"position":self.position.map(geo::data::geography_data::SpatialPositionData::from),"lifecycle":self.lifecycle.as_str(),"revision":self.revision,"input_revision":self.input_revision,"evidence":self.evidence.as_ref().map(location_evidence)}),
        )
    }
    fn decode(v: serde_json::Value) -> Result<Self, BoxError> {
        use application::idempotency as r;
        use party_core::party_location::*;
        Ok(Self {
            id: r::field(&v, "id")?,
            party_id: r::field(&v, "party_id")?,
            label: PartyLocationLabel::new(r::field(&v, "label")?)?,
            roles: r::field::<Vec<String>>(&v, "roles")?
                .into_iter()
                .map(|s| {
                    PartyLocationRole::from_code(&s)
                        .ok_or_else(|| application::error::static_error("invalid location role"))
                })
                .collect::<Result<_, _>>()?,
            geography: r::field::<Option<geo::data::geography_data::GeographicDescriptionData>>(
                &v,
                "geography",
            )?
            .map(TryInto::<Option<geo::GeographicDescription>>::try_into)
            .transpose()?
            .flatten(),
            position: r::field::<Option<geo::data::geography_data::SpatialPositionData>>(
                &v, "position",
            )?
            .map(TryInto::try_into)
            .transpose()?,
            lifecycle: PartyLocationLifecycle::from_code(&r::field::<String>(&v, "lifecycle")?)
                .ok_or_else(|| application::error::static_error("invalid lifecycle"))?,
            revision: r::field(&v, "revision")?,
            input_revision: r::field(&v, "input_revision")?,
            evidence: r::field::<Option<serde_json::Value>>(&v, "evidence")?
                .map(|v| {
                    let scope =
                        crate::use_cases::party_locations::LocationAssertionScope::from_code(
                            &r::field::<String>(&v, "scope")?,
                        )
                        .ok_or_else(|| {
                            application::error::static_error("invalid evidence scope")
                        })?;
                    Ok::<_, BoxError>(crate::use_cases::party_locations::LocationEvidence::new(
                        r::field(&v, "reference")?,
                        r::field::<Option<String>>(&v, "observed_at")?
                            .map(|v| v.parse::<i128>())
                            .transpose()?
                            .map(time::OffsetDateTime::from_unix_timestamp_nanos)
                            .transpose()?,
                        scope,
                    )?)
                })
                .transpose()?,
        })
    }
}
pub(crate) fn location_evidence(
    v: &crate::use_cases::party_locations::LocationEvidence,
) -> serde_json::Value {
    serde_json::json!({"reference":v.reference(),"scope":v.scope().as_str(),"observed_at":v.observed_at().map(|v|v.unix_timestamp_nanos().to_string())})
}

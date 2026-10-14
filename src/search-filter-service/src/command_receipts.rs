use crate::{
    ports::{SearchFilterMatchView, SearchFilterView},
    use_cases::{
        DeleteOwnedSearchFilterResult, UpdateOwnedSearchFilterResult,
        UpdateSearchFilterMatchFeedbackResult,
    },
};
use application::{
    error::{BoxError, static_error},
    idempotency::{self as retry, CommandOutcome},
};
use auction_core::AuctionId;
use domain_primitives::{object_id::ObjectIdError, query::range_query::RangeQuery};
use localization::Language;
use money::Currency;
use product_listing_core::{
    listing_availability::ListingAvailability,
    listing_orderability::ListingOrderability,
    product_listing_id::ProductListingId,
    product_listing_search::{
        EnhancedSearchDescription, ListingAvailabilityQuery, ProductListingSearch,
    },
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{collections::HashSet, hash::Hash};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

// Receipt-owned V1 payload. Keep its existing JSON shape independent of PostgreSQL row mapping.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SearchSnapshot {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    geography: Option<geo::data::discovery_data::GeographicQueryData>,
    language: String,
    currency: String,
    product_listing_query: Vec<domain_primitives::query::text_query::TextQuery<1>>,
    enhanced_search_description: Option<String>,
    exclude_product_listing_id_query: HashSet<String>,
    listing_source_id_query: HashSet<String>,
    exclude_listing_source_id_query: HashSet<String>,
    auction_id_query: HashSet<String>,
    price_query: Option<RangeQuery<u64>>,
    availability_query: Option<AvailabilitySnapshot>,
    created_query: Option<TimeRangeSnapshot>,
    updated_query: Option<TimeRangeSnapshot>,
    lot_bidding_opens_query: Option<TimeRangeSnapshot>,
    lot_scheduled_closes_query: Option<TimeRangeSnapshot>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct AvailabilitySnapshot {
    availability: HashSet<String>,
    orderability: HashSet<String>,
    include_unspecified: bool,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct TimeRangeSnapshot {
    min: Option<String>,
    max: Option<String>,
}

impl TimeRangeSnapshot {
    fn encode(range: RangeQuery<OffsetDateTime>) -> Result<Self, BoxError> {
        Ok(Self {
            min: range.min.map(|v| v.format(&Rfc3339)).transpose()?,
            max: range.max.map(|v| v.format(&Rfc3339)).transpose()?,
        })
    }

    fn decode(self) -> Result<RangeQuery<OffsetDateTime>, BoxError> {
        Ok(RangeQuery {
            min: self
                .min
                .map(|v| OffsetDateTime::parse(&v, &Rfc3339))
                .transpose()?,
            max: self
                .max
                .map(|v| OffsetDateTime::parse(&v, &Rfc3339))
                .transpose()?,
        })
    }
}

fn code<T>(value: &str, parse: fn(&str) -> Option<T>) -> Result<T, BoxError> {
    parse(value).ok_or_else(|| static_error("invalid search receipt code"))
}

fn codes<T: Eq + Hash>(
    values: HashSet<String>,
    parse: fn(&str) -> Option<T>,
) -> Result<HashSet<T>, BoxError> {
    values.into_iter().map(|v| code(&v, parse)).collect()
}

fn object_ids<T: Eq + Hash>(
    values: HashSet<String>,
    parse: fn(uuid::Uuid) -> Result<T, ObjectIdError>,
) -> Result<HashSet<T>, BoxError> {
    values
        .into_iter()
        .map(|value| {
            let id = uuid::Uuid::parse_str(&value)?;
            if id.to_string() != value {
                return Err(static_error(
                    "receipt UUID must use canonical hyphenated lowercase text",
                ));
            }
            Ok(parse(id)?)
        })
        .collect()
}

fn product_search_to_json(search: &ProductListingSearch) -> Result<Value, BoxError> {
    let snapshot = SearchSnapshot {
        geography: search.geography.as_ref().map(Into::into),
        language: search.language.as_str().to_owned(),
        currency: search.currency.as_str().to_owned(),
        product_listing_query: search.product_listing_query.clone(),
        enhanced_search_description: search
            .enhanced_search_description
            .as_ref()
            .map(ToString::to_string),
        exclude_product_listing_id_query: search
            .exclude_product_listing_id_query
            .iter()
            .map(|id| id.as_uuid().to_string())
            .collect(),
        listing_source_id_query: search
            .listing_source_id_query
            .iter()
            .map(|id| id.as_uuid().to_string())
            .collect(),
        exclude_listing_source_id_query: search
            .exclude_listing_source_id_query
            .iter()
            .map(|id| id.as_uuid().to_string())
            .collect(),
        auction_id_query: search
            .auction_id_query
            .iter()
            .map(|id| id.as_uuid().to_string())
            .collect(),
        price_query: search.price_query.map(|range| range.map(u64::from)),
        availability_query: search
            .availability_query
            .as_ref()
            .map(|query| AvailabilitySnapshot {
                availability: query.any_of.iter().map(|v| v.as_str().to_owned()).collect(),
                orderability: query
                    .orderability
                    .iter()
                    .map(|v| v.as_str().to_owned())
                    .collect(),
                include_unspecified: query.include_unspecified,
            }),
        created_query: search
            .created_query
            .map(TimeRangeSnapshot::encode)
            .transpose()?,
        updated_query: search
            .updated_query
            .map(TimeRangeSnapshot::encode)
            .transpose()?,
        lot_bidding_opens_query: search
            .lot_bidding_opens_query
            .map(TimeRangeSnapshot::encode)
            .transpose()?,
        lot_scheduled_closes_query: search
            .lot_scheduled_closes_query
            .map(TimeRangeSnapshot::encode)
            .transpose()?,
    };
    Ok(serde_json::to_value(snapshot)?)
}

fn product_search_from_json(value: Value) -> Result<ProductListingSearch, BoxError> {
    let snapshot: SearchSnapshot = serde_json::from_value(value)?;
    let availability_query = snapshot
        .availability_query
        .map(|query| {
            Ok::<_, BoxError>(ListingAvailabilityQuery {
                any_of: codes(query.availability, ListingAvailability::from_code)?.into(),
                orderability: codes(query.orderability, ListingOrderability::from_code)?.into(),
                include_unspecified: query.include_unspecified,
            })
        })
        .transpose()?;
    Ok(ProductListingSearch {
        geography: snapshot.geography.map(TryInto::try_into).transpose()?,
        language: code(&snapshot.language, Language::from_code)?,
        currency: code(&snapshot.currency, Currency::from_code)?,
        product_listing_query: snapshot.product_listing_query,
        enhanced_search_description: snapshot
            .enhanced_search_description
            .map(EnhancedSearchDescription::try_from)
            .transpose()?,
        exclude_product_listing_id_query: object_ids(
            snapshot.exclude_product_listing_id_query,
            ProductListingId::try_from,
        )?
        .into(),
        listing_source_id_query: object_ids(snapshot.listing_source_id_query, TryFrom::try_from)?
            .into(),
        exclude_listing_source_id_query: object_ids(
            snapshot.exclude_listing_source_id_query,
            TryFrom::try_from,
        )?
        .into(),
        auction_id_query: object_ids(snapshot.auction_id_query, AuctionId::try_from)?.into(),
        price_query: snapshot.price_query.map(|range| range.map(Into::into)),
        availability_query,
        created_query: snapshot
            .created_query
            .map(TimeRangeSnapshot::decode)
            .transpose()?,
        updated_query: snapshot
            .updated_query
            .map(TimeRangeSnapshot::decode)
            .transpose()?,
        lot_bidding_opens_query: snapshot
            .lot_bidding_opens_query
            .map(TimeRangeSnapshot::decode)
            .transpose()?,
        lot_scheduled_closes_query: snapshot
            .lot_scheduled_closes_query
            .map(TimeRangeSnapshot::decode)
            .transpose()?,
    })
}

impl CommandOutcome for SearchFilterView {
    fn private_owner(&self) -> Option<user_core::user_id::UserId> {
        Some(self.user_id)
    }
    fn encode(&self) -> Result<Value, BoxError> {
        Ok(
            json!({"search_filter_id":self.search_filter_id,"user_id":self.user_id,"name":self.name.as_ref(),"notifications":self.notifications,"state":self.state.as_str(),"search":product_search_to_json(&self.search)?,"embedding":self.embedding,"created":retry::time_value(self.created)?,"updated":retry::time_value(self.updated)?}),
        )
    }
    fn decode(v: Value) -> Result<Self, BoxError> {
        let name: String = retry::field(&v, "name")?;
        if name.len() > 255 {
            return Err(static_error("invalid filter name"));
        }
        let embedding: Option<Vec<f32>> = retry::field(&v, "embedding")?;
        if embedding
            .as_ref()
            .is_some_and(|v| v.len() != 768 || v.iter().any(|v| !v.is_finite()))
        {
            return Err(static_error("invalid embedding"));
        }
        Ok(Self {
            search_filter_id: retry::field(&v, "search_filter_id")?,
            user_id: retry::field(&v, "user_id")?,
            name: name.into(),
            notifications: retry::field(&v, "notifications")?,
            state: search_filter_core::search_filter_state::SearchFilterState::from_code(
                &retry::field::<String>(&v, "state")?,
            )
            .ok_or_else(|| static_error("invalid filter state"))?,
            search: product_search_from_json(retry::field(&v, "search")?)?,
            embedding,
            created: retry::timestamp(&v, "created")?,
            updated: retry::timestamp(&v, "updated")?,
        })
    }
}
impl CommandOutcome for UpdateOwnedSearchFilterResult {
    fn private_owner(&self) -> Option<user_core::user_id::UserId> {
        Some(self.filter.user_id)
    }
    fn encode(&self) -> Result<Value, BoxError> {
        self.filter.encode()
    }
    fn decode(v: Value) -> Result<Self, BoxError> {
        Ok(Self {
            filter: SearchFilterView::decode(v)?,
        })
    }
}
application::command_outcome!(unit DeleteOwnedSearchFilterResult);
impl CommandOutcome for UpdateSearchFilterMatchFeedbackResult {
    fn private_owner(&self) -> Option<user_core::user_id::UserId> {
        Some(self.search_filter_match.user_id)
    }
    fn encode(&self) -> Result<Value, BoxError> {
        let v = &self.search_filter_match;
        Ok(
            json!({"user_id":v.user_id,"search_filter_id":v.search_filter_id,"search_filter_name":v.search_filter_name.as_ref().map(|v|v.as_ref()),"product_listing_id":v.product_listing_id,"origin_event_id":v.origin_event_id,"feedback":v.feedback,"created":retry::time_value(v.created)?,"updated":retry::time_value(v.updated)?}),
        )
    }
    fn decode(v: Value) -> Result<Self, BoxError> {
        let name: Option<String> = retry::field(&v, "search_filter_name")?;
        if name.as_ref().is_some_and(|s| s.len() > 255) {
            return Err(static_error("invalid filter name"));
        }
        Ok(Self {
            search_filter_match: SearchFilterMatchView {
                user_id: retry::field(&v, "user_id")?,
                search_filter_id: retry::field(&v, "search_filter_id")?,
                search_filter_name: name.map(Into::into),
                product_listing_id: retry::field(&v, "product_listing_id")?,
                origin_event_id: retry::field(&v, "origin_event_id")?,
                feedback: retry::field(&v, "feedback")?,
                created: retry::timestamp(&v, "created")?,
                updated: retry::timestamp(&v, "updated")?,
            },
        })
    }
}

pub(crate) fn update_intent(command: &crate::use_cases::UpdateOwnedSearchFilterCommand) -> Value {
    use crate::use_cases::ProductListingSearchPatch;
    use application::patch_field::PatchField;
    let ProductListingSearchPatch {
        geography,
        language,
        currency,
        product_listing_query,
        enhanced_search_description,
        listing_source_id_query,
        exclude_listing_source_id_query,
        auction_id_query,
        price_query,
        availability_query,
        created_query,
        updated_query,
        lot_bidding_opens_query,
        lot_scheduled_closes_query,
    } = &command.search;
    let mut search = serde_json::Map::new();
    macro_rules! leaf {($field:ident)=>{{let value=match $field {PatchField::Unchanged=>json!({"kind":"UNCHANGED"}),PatchField::Clear=>json!({"kind":"CLEAR"}),PatchField::Set(value)=>{let mut candidate=ProductListingSearch::new(Language::En,Currency::Eur);candidate.$field=value.clone().into();let intent=crate::create_receipt::search_value(&candidate);json!({"kind":"SET","value":intent[stringify!($field)].clone()})}};search.insert(stringify!($field).into(),value);}};}
    leaf!(geography);
    leaf!(language);
    leaf!(currency);
    leaf!(product_listing_query);
    leaf!(enhanced_search_description);
    leaf!(listing_source_id_query);
    leaf!(exclude_listing_source_id_query);
    leaf!(auction_id_query);
    leaf!(price_query);
    leaf!(availability_query);
    leaf!(created_query);
    leaf!(updated_query);
    leaf!(lot_bidding_opens_query);
    leaf!(lot_scheduled_closes_query);
    json!({"user_id":command.user_id,"search_filter_id":command.search_filter_id,"name":retry::patch(&command.name,|v|json!(v.as_ref())),"notifications":retry::patch(&command.notifications,|v|json!(v)),"state":retry::patch(&command.state,|v|json!(v.as_str())),"search":search})
}

#[cfg(test)]
mod tests {
    use super::{product_search_from_json, product_search_to_json};
    use serde_json::{Value, json};

    fn snapshot() -> Value {
        let id = "01890a5d-ac96-774b-bf9d-d5587833efb5";
        json!({
            "language": "en",
            "currency": "EUR",
            "product_listing_query": ["desk", "chair"],
            "enhanced_search_description": "antique furniture",
            "exclude_product_listing_id_query": [id],
            "listing_source_id_query": [id],
            "exclude_listing_source_id_query": [id],
            "auction_id_query": [id],
            "price_query": {"min": 100},
            "availability_query": {
                "availability": ["IN_STOCK"],
                "orderability": ["ORDERABLE_NOW"],
                "include_unspecified": true
            },
            "created_query": {"min": "1970-01-01T00:00:00Z", "max": null},
            "updated_query": {"min": null, "max": "2026-10-14T11:00:00Z"},
            "lot_bidding_opens_query": {"min": null, "max": null},
            "lot_scheduled_closes_query": null
        })
    }

    #[test]
    fn search_snapshot_preserves_the_v1_receipt_format() {
        let original = snapshot();
        let search = product_search_from_json(original.clone()).unwrap();
        assert_eq!(original, product_search_to_json(&search).unwrap());

        let empty = product_listing_core::product_listing_search::ProductListingSearch::new(
            localization::Language::De,
            money::Currency::Usd,
        );
        assert_eq!(
            empty,
            product_search_from_json(product_search_to_json(&empty).unwrap()).unwrap()
        );

        let mut with_geography = search;
        with_geography.geography = Some(
            geo::core::discovery::GeographicQuery::new(
                geo::reference::ReferenceRelease::CURRENT,
                Default::default(),
                Default::default(),
                Some(geo::GeoPoint::new(48., 11.).unwrap()),
                Some(geo::Distance::new(1., geo::DistanceUnit::Kilometers).unwrap()),
                geo::core::discovery::ProximityMode::Strict,
                None,
            )
            .unwrap(),
        );
        assert_eq!(
            with_geography,
            product_search_from_json(product_search_to_json(&with_geography).unwrap()).unwrap()
        );
    }

    #[test]
    fn search_snapshot_rejects_invalid_or_noncanonical_state() {
        for (pointer, invalid) in [
            ("/language", json!("EN")),
            ("/currency", json!("eur")),
            ("/product_listing_query", json!([""])),
            ("/enhanced_search_description", json!(" ")),
            ("/availability_query/availability", json!(["in_stock"])),
            ("/availability_query/orderability", json!(["UNKNOWN"])),
            ("/created_query/min", json!("not-a-time")),
            ("/updated_query/max", json!("not-a-time")),
            ("/lot_bidding_opens_query/min", json!("not-a-time")),
        ] {
            let mut value = snapshot();
            *value.pointer_mut(pointer).unwrap() = invalid;
            assert!(product_search_from_json(value).is_err(), "{pointer}");
        }
        for field in [
            "exclude_product_listing_id_query",
            "listing_source_id_query",
            "exclude_listing_source_id_query",
            "auction_id_query",
        ] {
            for invalid in [
                "not-a-uuid",
                "01890A5D-AC96-774B-BF9D-D5587833EFB5",
                "01890a5dac96774bbf9dd5587833efb5",
                "00000000-0000-0000-0000-000000000000",
            ] {
                let mut value = snapshot();
                value[field] = json!([invalid]);
                assert!(
                    product_search_from_json(value).is_err(),
                    "{field}: {invalid}"
                );
            }
        }
        let mut value = snapshot();
        value["unexpected"] = json!(true);
        assert!(product_search_from_json(value).is_err());
        let mut value = snapshot();
        value["created_query"]["unexpected"] = json!(true);
        assert!(product_search_from_json(value).is_err());
        let mut value = snapshot();
        value.as_object_mut().unwrap().remove("language");
        assert!(product_search_from_json(value).is_err());
    }
}

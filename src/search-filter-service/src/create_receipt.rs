use crate::use_cases::{CreateSearchFilterCommand, CreateSearchFilterResult};
use application::idempotency::{CommandFingerprint, IdempotencyKey};
use application::operation_context::{OperationContext, Principal};
use domain_primitives::query::range_query::RangeQuery;
use serde_json::json;
use time::OffsetDateTime;
use user_core::user_id::UserId;

/// Owner and trusted actor scope. Refreshed and delegated user credentials share identity;
/// service/system callers have independent namespaces. Request IDs never enter this identity.
#[derive(Clone)]
pub struct CreateSearchFilterIntent {
    pub user_id: UserId,
    pub actor: String,
    pub key: IdempotencyKey,
    pub fingerprint: CommandFingerprint,
}

impl CreateSearchFilterIntent {
    /// A decoded original outcome must still describe the bound submitted creation intent.
    pub fn matches_result(&self, result: &CreateSearchFilterResult) -> bool {
        let view = &result.filter;
        self.fingerprint
            == fingerprint(&CreateSearchFilterCommand {
                idempotency_key: None,
                user_id: view.user_id,
                name: view.name.clone(),
                notifications: view.notifications,
                search: view.search.clone(),
            })
    }

    pub(crate) fn new(
        context: &OperationContext,
        command: &CreateSearchFilterCommand,
        key: IdempotencyKey,
    ) -> Self {
        let actor = match &context.principal {
            Principal::User(id) | Principal::DelegatedUser { user_id: id, .. } => {
                format!("USER:{id}")
            }
            Principal::Service(id) => format!("SERVICE:{id}"),
            Principal::System => "SYSTEM".into(),
            Principal::Anonymous => unreachable!("receipt identity follows owner authorization"),
        };
        Self {
            user_id: command.user_id,
            actor,
            key,
            fingerprint: fingerprint(command),
        }
    }
}

fn sorted(values: impl Iterator<Item = String>) -> Vec<String> {
    let mut values: Vec<_> = values.collect();
    values.sort_unstable();
    values
}

fn timestamps(range: Option<RangeQuery<OffsetDateTime>>) -> Option<RangeQuery<String>> {
    range.map(|r| r.map(|t| t.unix_timestamp_nanos().to_string()))
}

fn fingerprint(command: &CreateSearchFilterCommand) -> CommandFingerprint {
    let mut intent = json!({"user_id":command.user_id.to_string(),"name":command.name.as_ref(),"notifications":command.notifications,"search":search_value(&command.search)});
    // Pin object ordering even if another workspace consumer enables serde_json/preserve_order.
    intent.sort_all_objects();
    CommandFingerprint::digest(1, intent.to_string().as_bytes())
}

pub(crate) fn search_value(
    search: &product_listing_core::product_listing_search::ProductListingSearch,
) -> serde_json::Value {
    // Exhaustive matching requires future search fields to make an explicit fingerprint decision.
    let product_listing_core::product_listing_search::ProductListingSearch {
        geography,
        language,
        currency,
        product_listing_query,
        enhanced_search_description,
        exclude_product_listing_id_query,
        listing_source_id_query,
        exclude_listing_source_id_query,
        auction_id_query,
        price_query,
        availability_query,
        created_query,
        updated_query,
        lot_bidding_opens_query,
        lot_scheduled_closes_query,
    } = search;
    let geography = geography.as_ref().map(|g| {
        json!({
            "reference_release": g.reference_release().as_str(),
            "countries": sorted(g.countries().iter().map(|v| v.alpha2().to_owned())),
            "subdivisions": sorted(g.subdivisions().iter().map(|v| v.as_str().to_owned())),
            "origin": g.origin().map(|p| (p.latitude_degrees(), p.longitude_degrees())),
            "radius": g.radius().map(|r| (r.amount(), r.unit().as_str())),
            "mode": g.mode().as_str(),
            "distance_order": g.distance_order().map(|v| v.as_str()),
        })
    });
    json!({
        "geography": geography,
        "language": language.as_str(),
        "currency": currency.as_str(),
        "product_listing_query": product_listing_query,
        "enhanced_search_description": enhanced_search_description.as_ref().map(|v| v.as_ref()),
        "exclude_product_listing_id_query": sorted(exclude_product_listing_id_query.iter().map(ToString::to_string)),
        "listing_source_id_query": sorted(listing_source_id_query.iter().map(ToString::to_string)),
        "exclude_listing_source_id_query": sorted(exclude_listing_source_id_query.iter().map(ToString::to_string)),
        "auction_id_query": sorted(auction_id_query.iter().map(ToString::to_string)),
        "price_query": price_query.map(|r| r.map(u64::from)),
        "availability_query": availability_query.as_ref().map(|q| json!({
            "any_of": sorted(q.any_of.iter().map(|v| v.as_str().to_owned())),
            "orderability": sorted(q.orderability.iter().map(|v| v.as_str().to_owned())),
            "include_unspecified": q.include_unspecified,
        })),
        "created_query": timestamps(*created_query),
        "updated_query": timestamps(*updated_query),
        "lot_bidding_opens_query": timestamps(*lot_bidding_opens_query),
        "lot_scheduled_closes_query": timestamps(*lot_scheduled_closes_query),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use listing_source_core::ListingSourceId;
    use localization::Language;
    use money::Currency;
    use product_listing_core::product_listing_search::ProductListingSearch;

    fn command() -> CreateSearchFilterCommand {
        CreateSearchFilterCommand {
            idempotency_key: None,
            user_id: UserId::try_from("usr_01h455vb4pex5vy7enb1p677vn").unwrap(),
            name: "desk".into(),
            notifications: true,
            search: ProductListingSearch::new(Language::En, Currency::Eur),
        }
    }

    #[test]
    fn v1_fingerprint_encoding_remains_compatible_across_deployments() {
        assert_eq!(
            &[
                136, 63, 108, 122, 212, 66, 167, 221, 215, 216, 227, 37, 191, 74, 245, 234, 41, 99,
                200, 243, 252, 251, 209, 30, 217, 188, 106, 3, 32, 221, 203, 169
            ],
            fingerprint(&command()).as_bytes()
        );
    }

    #[test]
    fn sets_are_canonical_but_text_array_order_and_present_ranges_are_significant() {
        let a = ListingSourceId::new();
        let b = ListingSourceId::new();
        let mut one = command();
        one.search.listing_source_id_query.extend([a, b]);
        let mut two = command();
        two.search.listing_source_id_query.extend([b, a]);
        assert_eq!(fingerprint(&one), fingerprint(&two));
        one.search.product_listing_query =
            vec!["first".try_into().unwrap(), "second".try_into().unwrap()];
        two.search.product_listing_query = one.search.product_listing_query.clone();
        assert_eq!(fingerprint(&one), fingerprint(&two));
        two.search.product_listing_query.reverse();
        assert_ne!(fingerprint(&one), fingerprint(&two));
        two = one.clone();
        two.search.price_query = Some(Default::default());
        assert_ne!(fingerprint(&one), fingerprint(&two));
    }

    #[test]
    fn key_and_request_metadata_do_not_change_intent_but_business_fields_do() {
        let original = command();
        let mut changed = original.clone();
        changed.idempotency_key = Some(IdempotencyKey::new("different-key".into()).unwrap());
        assert_eq!(fingerprint(&original), fingerprint(&changed));
        changed.notifications = false;
        assert_ne!(fingerprint(&original), fingerprint(&changed));
        changed = original.clone();
        changed.name = "different".into();
        assert_ne!(fingerprint(&original), fingerprint(&changed));
        changed = original.clone();
        changed.search.language = Language::De;
        assert_ne!(fingerprint(&original), fingerprint(&changed));
        changed = original.clone();
        changed.search.currency = Currency::Usd;
        assert_ne!(fingerprint(&original), fingerprint(&changed));
        changed = original.clone();
        changed.search.enhanced_search_description = Some("originals".try_into().unwrap());
        assert_ne!(fingerprint(&original), fingerprint(&changed));
        changed = original.clone();
        changed.search.created_query = Some(RangeQuery {
            min: Some(OffsetDateTime::UNIX_EPOCH),
            max: None,
        });
        assert_ne!(fingerprint(&original), fingerprint(&changed));
    }

    #[test]
    fn geography_binds_origin_radius_units_mode_and_release_pinned_codes() {
        use geo::core::discovery::{GeographicQuery, ProximityMode};
        let mut one = command();
        one.search.geography = Some(
            GeographicQuery::new(
                geo::reference::ReferenceRelease::CURRENT,
                Default::default(),
                Default::default(),
                Some(geo::GeoPoint::new(48., 11.).unwrap()),
                Some(geo::Distance::new(1., geo::DistanceUnit::Kilometers).unwrap()),
                ProximityMode::Strict,
                None,
            )
            .unwrap(),
        );
        let mut two = one.clone();
        two.search.geography = Some(
            GeographicQuery::new(
                geo::reference::ReferenceRelease::CURRENT,
                Default::default(),
                Default::default(),
                Some(geo::GeoPoint::new(48., 11.).unwrap()),
                Some(geo::Distance::new(1., geo::DistanceUnit::Miles).unwrap()),
                ProximityMode::Strict,
                None,
            )
            .unwrap(),
        );
        assert_ne!(fingerprint(&one), fingerprint(&two));
        two.search.geography = Some(
            GeographicQuery::new(
                geo::reference::ReferenceRelease::CURRENT,
                Default::default(),
                Default::default(),
                Some(geo::GeoPoint::new(48.1, 11.).unwrap()),
                Some(geo::Distance::new(1., geo::DistanceUnit::Kilometers).unwrap()),
                ProximityMode::Strict,
                None,
            )
            .unwrap(),
        );
        assert_ne!(fingerprint(&one), fingerprint(&two));
    }
}

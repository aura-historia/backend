use geo::core::discovery::{GeographicQuery, ProximityMode};
use serde_json::{Value, json};

/// Effective geography selects one location per meaning; each object preserves that association.
pub(crate) fn clauses(field: &str, q: &GeographicQuery) -> Vec<Value> {
    let mut filters = vec![
        json!({"term": {format!("{field}.referenceRelease"): q.reference_release().as_str()}}),
    ];
    if !q.countries().is_empty() {
        filters.push(json!({"terms": {format!("{field}.country"): q.countries().iter().map(|c| c.alpha2()).collect::<Vec<_>>()}}));
    }
    if !q.subdivisions().is_empty() {
        filters.push(json!({"terms": {format!("{field}.subdivision"): q.subdivisions().iter().map(|s| s.as_str()).collect::<Vec<_>>()}}));
    }
    if let Some(origin) = q.origin() {
        filters.push(json!({"exists": {"field": format!("{field}.point")}}));
        let precisions = match q.mode() {
            ProximityMode::Strict => vec!["PREMISES"],
            ProximityMode::Approximate => vec!["PREMISES", "STREET", "LOCALITY", "REPRESENTATIVE"],
        };
        filters.push(json!({"terms": {format!("{field}.precision"): precisions}}));
        if let Some(radius) = q.radius() {
            filters.push(json!({"geo_distance": {"distance": geo::opensearch::distance_to_opensearch_value(radius), "distance_type": "arc", format!("{field}.point"): {"lat": origin.latitude_degrees(), "lon": origin.longitude_degrees()}}}));
        }
    }
    filters
}
pub(crate) fn distance_sort(field: &str, q: &GeographicQuery) -> Option<Value> {
    let origin = q.origin()?;
    let order = q.distance_order()?;
    Some(
        json!({"_geo_distance": {format!("{field}.point"): {"lat": origin.latitude_degrees(), "lon": origin.longitude_degrees()}, "order": order.as_str().to_ascii_lowercase(), "unit": "km", "distance_type": "arc", "mode": "min"}}),
    )
}

//! Geographic predicates shared by listing and public location discovery.
use crate::reference::ReferenceRelease;
use crate::{CountryCode, GeoPoint, SpatialPosition, SpatialPrecision, SubdivisionCode};
use std::collections::HashSet;

pub const MAX_RADIUS_KM: f64 = 20_000.0;
pub const EARTH_RADIUS_KM: f64 = 6_371.008_771_4;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ProximityMode {
    #[default]
    Strict,
    Approximate,
}
impl ProximityMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Strict => "STRICT",
            Self::Approximate => "APPROXIMATE",
        }
    }
    pub fn from_code(value: &str) -> Option<Self> {
        match value {
            "STRICT" => Some(Self::Strict),
            "APPROXIMATE" => Some(Self::Approximate),
            _ => None,
        }
    }
    pub fn eligible(self, precision: SpatialPrecision) -> bool {
        self == Self::Approximate || precision == SpatialPrecision::Premises
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DistanceOrder {
    Asc,
    Desc,
}
impl DistanceOrder {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Asc => "ASC",
            Self::Desc => "DESC",
        }
    }
    pub fn from_code(value: &str) -> Option<Self> {
        match value {
            "ASC" => Some(Self::Asc),
            "DESC" => Some(Self::Desc),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum InvalidGeographicQuery {
    #[error("geographic query must contain a country, subdivision, radius or distance order")]
    Empty,
    #[error(
        "geographic code sets are limited to 100 entries and subdivisions must match the country set and reference release"
    )]
    Codes,
    #[error("radius must be finite, greater than 0 and at most 20000 kilometres")]
    Radius,
    #[error(
        "proximity requires an explicit numeric coordinate origin; an origin or approximate mode requires radius or distance ordering"
    )]
    Origin,
}

/// Absence never matches a supplied predicate. Sets are OR; distinct predicates are AND.
/// An origin is a private immutable caller snapshot, with no address/profile reference.
#[derive(Clone, PartialEq)]
pub struct GeographicQuery {
    release: ReferenceRelease,
    countries: HashSet<CountryCode>,
    subdivisions: HashSet<SubdivisionCode>,
    origin: Option<GeoPoint>,
    radius_km: Option<f64>,
    mode: ProximityMode,
    distance_order: Option<DistanceOrder>,
}
impl std::fmt::Debug for GeographicQuery {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("GeographicQuery(<private predicates>)")
    }
}
impl GeographicQuery {
    pub fn new(
        release: ReferenceRelease,
        countries: HashSet<CountryCode>,
        subdivisions: HashSet<SubdivisionCode>,
        origin: Option<GeoPoint>,
        radius_km: Option<f64>,
        mode: ProximityMode,
        distance_order: Option<DistanceOrder>,
    ) -> Result<Self, InvalidGeographicQuery> {
        if countries.len() > 100
            || subdivisions.len() > 100
            || subdivisions.iter().any(|s| {
                s.reference_release() != release
                    || (!countries.is_empty() && !countries.contains(&s.country()))
            })
        {
            return Err(InvalidGeographicQuery::Codes);
        }
        if radius_km.is_some_and(|r| !r.is_finite() || r <= 0.0 || r > MAX_RADIUS_KM) {
            return Err(InvalidGeographicQuery::Radius);
        }
        let proximity = radius_km.is_some() || distance_order.is_some();
        if proximity != origin.is_some() || (!proximity && mode != ProximityMode::Strict) {
            return Err(InvalidGeographicQuery::Origin);
        }
        if countries.is_empty() && subdivisions.is_empty() && !proximity {
            return Err(InvalidGeographicQuery::Empty);
        }
        Ok(Self {
            release,
            countries,
            subdivisions,
            origin,
            radius_km,
            mode,
            distance_order,
        })
    }
    pub fn reference_release(&self) -> ReferenceRelease {
        self.release
    }
    pub fn countries(&self) -> &HashSet<CountryCode> {
        &self.countries
    }
    pub fn subdivisions(&self) -> &HashSet<SubdivisionCode> {
        &self.subdivisions
    }
    pub fn origin(&self) -> Option<GeoPoint> {
        self.origin
    }
    pub fn radius_km(&self) -> Option<f64> {
        self.radius_km
    }
    pub fn mode(&self) -> ProximityMode {
        self.mode
    }
    pub fn distance_order(&self) -> Option<DistanceOrder> {
        self.distance_order
    }
    pub fn matches(
        &self,
        country: Option<CountryCode>,
        subdivision: Option<&SubdivisionCode>,
        position: Option<SpatialPosition>,
    ) -> bool {
        (self.countries.is_empty() || country.is_some_and(|c| self.countries.contains(&c)))
            && (self.subdivisions.is_empty()
                || subdivision.is_some_and(|s| self.subdivisions.contains(s)))
            && self.origin.is_none_or(|origin| {
                position.is_some_and(|p| {
                    self.mode.eligible(p.precision())
                        && self
                            .radius_km
                            .is_none_or(|r| distance_km(origin, p.point()) <= r)
                })
            })
    }
}

/// Spherical arc distance to the recorded point, never journey distance or custody proof.
pub fn distance_km(a: GeoPoint, b: GeoPoint) -> f64 {
    let lat_a = a.latitude_degrees().to_radians();
    let lat_b = b.latitude_degrees().to_radians();
    let dlat = (b.latitude_degrees() - a.latitude_degrees()).to_radians();
    let dlon = (b.longitude_degrees() - a.longitude_degrees()).to_radians();
    let h = (dlat / 2.0).sin().powi(2) + lat_a.cos() * lat_b.cos() * (dlon / 2.0).sin().powi(2);
    2.0 * EARTH_RADIUS_KM * h.clamp(0.0, 1.0).sqrt().asin()
}

/// Public display precision follows evidence quality; sorting uses the original distance.
pub fn display_distance_km(distance: f64, precision: SpatialPrecision) -> f64 {
    let scale = match precision {
        SpatialPrecision::Premises => 10.0,
        SpatialPrecision::Street => 1.0,
        SpatialPrecision::Locality | SpatialPrecision::Representative => 0.1,
    };
    (distance * scale).round() / scale
}

#[cfg(test)]
mod tests {
    use super::*;
    fn query(
        origin: Option<GeoPoint>,
        radius: Option<f64>,
        mode: ProximityMode,
    ) -> Result<GeographicQuery, InvalidGeographicQuery> {
        GeographicQuery::new(
            ReferenceRelease::CURRENT,
            HashSet::new(),
            HashSet::new(),
            origin,
            radius,
            mode,
            None,
        )
    }
    fn point(lat: f64, lon: f64) -> GeoPoint {
        GeoPoint::new(lat, lon).unwrap()
    }
    #[test]
    fn germany_bavaria_requires_no_point_and_never_invents_missing_geography() {
        let bavaria = SubdivisionCode::new("DE-BY").unwrap();
        let q = GeographicQuery::new(
            ReferenceRelease::CURRENT,
            HashSet::from([CountryCode::DEU]),
            HashSet::from([bavaria]),
            None,
            None,
            ProximityMode::Strict,
            None,
        )
        .unwrap();
        assert!(q.matches(Some(CountryCode::DEU), Some(&bavaria), None));
        assert!(!q.matches(None, Some(&bavaria), None));
        assert!(!q.matches(Some(CountryCode::DEU), None, None));
        assert!(
            GeographicQuery::new(
                ReferenceRelease::CURRENT,
                HashSet::from([CountryCode::FRA]),
                HashSet::from([bavaria]),
                None,
                None,
                ProximityMode::Strict,
                None
            )
            .is_err()
        );
    }
    #[test]
    fn origins_and_radius_have_explicit_bounds_and_combinations() {
        assert!(query(None, Some(25.0), ProximityMode::Strict).is_err());
        assert!(query(Some(point(0.0, 0.0)), None, ProximityMode::Strict).is_err());
        for radius in [f64::NAN, f64::INFINITY, -1.0, 0.0, MAX_RADIUS_KM + 1.0] {
            assert!(query(Some(point(0.0, 0.0)), Some(radius), ProximityMode::Strict).is_err());
        }
        assert!(
            query(
                Some(point(-90.0, -180.0)),
                Some(MAX_RADIUS_KM),
                ProximityMode::Strict
            )
            .is_ok()
        );
        for (lat, lon) in [
            (90.1, 0.0),
            (0.0, 180.1),
            (f64::NAN, 0.0),
            (0.0, f64::INFINITY),
        ] {
            assert!(GeoPoint::new(lat, lon).is_err());
        }
    }
    #[test]
    fn strict_proximity_excludes_coarse_points_and_radius_is_inclusive() {
        let origin = point(48.137, 11.575);
        let target = point(48.3, 11.575);
        let exact = distance_km(origin, target);
        let q = query(Some(origin), Some(exact), ProximityMode::Strict).unwrap();
        let premises = SpatialPosition::new(target, SpatialPrecision::Premises, None).unwrap();
        assert!(q.matches(None, None, Some(premises)));
        assert!(
            !query(Some(origin), Some(exact - 0.00001), ProximityMode::Strict)
                .unwrap()
                .matches(None, None, Some(premises))
        );
        assert!(!q.matches(None, None, None));
        for precision in [
            SpatialPrecision::Street,
            SpatialPrecision::Locality,
            SpatialPrecision::Representative,
        ] {
            let coarse = SpatialPosition::new(target, precision, None).unwrap();
            assert!(!q.matches(None, None, Some(coarse)));
            assert!(
                query(Some(origin), Some(exact), ProximityMode::Approximate)
                    .unwrap()
                    .matches(None, None, Some(coarse))
            );
        }
    }
    #[test]
    fn arc_distance_handles_antimeridian_poles_zero_and_coordinate_order() {
        assert_eq!(0.0, distance_km(point(0.0, 0.0), point(0.0, 0.0)));
        assert!((distance_km(point(0.0, 179.9), point(0.0, -179.9)) - 22.239).abs() < 0.001);
        assert!(distance_km(point(90.0, -180.0), point(90.0, 180.0)) < 1e-8);
        assert!(distance_km(point(48.137, 11.575), point(11.575, 48.137)) > 4000.0);
        assert!(distance_km(point(-90.0, 0.0), point(90.0, 0.0)) > MAX_RADIUS_KM);
    }
}

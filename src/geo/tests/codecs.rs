#![cfg(feature = "data")]

use geo::{
    AddressText, CountryCode, Distance, DistanceUnit, GeoPoint, GeographicDescription, PostalCode,
    SpatialPosition, SpatialPrecision, SubdivisionCode,
    data::geography_data::{
        DistanceData, GeoPointData, GeographicDescriptionData, SpatialPositionData,
    },
    reference::ReferenceRelease,
};
use serde_json::{Value, json};

fn description_json() -> Value {
    json!({"reference_release": ReferenceRelease::CURRENT.as_str()})
}

fn decode(value: Value) -> Result<Option<GeographicDescription>, Box<dyn std::error::Error>> {
    let data: GeographicDescriptionData = serde_json::from_value(value)?;
    Ok(data.try_into()?)
}

#[test]
fn should_round_trip_exact_text_canonical_codes_and_reference_release() {
    let description = GeographicDescription::new(
        Some(AddressText::new("  東京都\r\n千代田区\n丸の内  ").unwrap()),
        Some(CountryCode::JPN),
        Some(SubdivisionCode::new("JP-13").unwrap()),
        Some(PostalCode::new("00123-0045").unwrap()),
    )
    .unwrap()
    .unwrap();
    let json = serde_json::to_value(GeographicDescriptionData::from(&description)).unwrap();
    assert_eq!("JP", json["country"]);
    assert_eq!("JP-13", json["subdivision"]);
    assert_eq!("iso-codes-4.20.1", json["reference_release"]);
    assert_eq!(Some(description), decode(json).unwrap());
    assert_eq!(None, decode(description_json()).unwrap());
}

#[test]
fn should_round_trip_release_for_each_partial_description() {
    for fields in [
        json!({"country": "DE"}),
        json!({"country": "DE", "subdivision": "DE-BE"}),
        json!({"subdivision": "DE-BE"}),
        json!({"address_text": "source text"}),
        json!({"postal_code": "00123"}),
    ] {
        let mut json = description_json();
        json.as_object_mut()
            .unwrap()
            .extend(fields.as_object().unwrap().clone());
        let description = decode(json).unwrap().unwrap();
        assert_eq!(ReferenceRelease::CURRENT, description.reference_release());
        let encoded = serde_json::to_value(GeographicDescriptionData::from(&description)).unwrap();
        assert_eq!(
            description.reference_release().as_str(),
            encoded["reference_release"]
        );
        for (key, expected) in fields.as_object().unwrap() {
            assert_eq!(expected, &encoded[key]);
        }
        assert_eq!(Some(description), decode(encoded).unwrap());
    }
}

#[test]
fn should_fail_closed_on_corrupt_persisted_assertions() {
    for (field, values) in [
        (
            "country",
            vec![
                "jp".to_owned(),
                " J P".to_owned(),
                "JPN".to_owned(),
                "ZZ".to_owned(),
                "UK".to_owned(),
                "XK".to_owned(),
                "EU".to_owned(),
            ],
        ),
        (
            "subdivision",
            vec![
                "13".to_owned(),
                "JP-999".to_owned(),
                "jp-13".to_owned(),
                "JP-13 ".to_owned(),
            ],
        ),
        (
            "address_text",
            vec![
                " ".to_owned(),
                "a\0b".to_owned(),
                "a".repeat(AddressText::MAX_BYTES + 1),
            ],
        ),
        (
            "postal_code",
            vec![
                " ".to_owned(),
                "001\n23".to_owned(),
                "0".repeat(PostalCode::MAX_BYTES + 1),
            ],
        ),
        (
            "reference_release",
            vec!["latest".to_owned(), "iso-codes-4.21.0".to_owned()],
        ),
    ] {
        for value in values {
            let mut json = description_json();
            json[field] = json!(value);
            assert!(decode(json).is_err(), "{field}");
        }
    }
    for invalid in [
        json!({"country": "JP"}),
        json!({"reference_release": null, "country": "JP"}),
        json!({"reference_release": ReferenceRelease::CURRENT.as_str(), "country": "DE", "subdivision": "JP-13"}),
        json!({"reference_release": ReferenceRelease::CURRENT.as_str(), "country": "JP", "unexpected": true}),
    ] {
        assert!(decode(invalid).is_err());
    }
}

#[test]
fn should_validate_numeric_json_during_boundary_rehydration() {
    for json in [
        r#"{"latitude_degrees":91,"longitude_degrees":0}"#,
        r#"{"latitude_degrees":0,"longitude_degrees":-181}"#,
    ] {
        assert!(GeoPoint::try_from(serde_json::from_str::<GeoPointData>(json).unwrap()).is_err());
    }
    for json in [
        r#"{"amount":-1,"unit":"METERS"}"#,
        r#"{"amount":1,"unit":"m"}"#,
    ] {
        assert!(Distance::try_from(serde_json::from_str::<DistanceData>(json).unwrap()).is_err());
    }
    for (precision, accuracy) in [("LOCALITY", Some(-1.0)), ("locality", None)] {
        let data = serde_json::from_value::<SpatialPositionData>(json!({
            "point": {"latitude_degrees": 0, "longitude_degrees": 0},
            "precision": precision,
            "accuracy_metres": accuracy,
        }))
        .unwrap();
        assert!(SpatialPosition::try_from(data).is_err());
    }
}

#[test]
fn should_round_trip_explicit_coordinate_order_precision_accuracy_and_units() {
    let point = GeoPoint::new(52.52, 13.405).unwrap();
    let data = GeoPointData::from(point);
    let json = serde_json::to_value(data).unwrap();
    assert_eq!(52.52, json["latitude_degrees"]);
    assert_eq!(13.405, json["longitude_degrees"]);
    assert_eq!(point, GeoPoint::try_from(data).unwrap());
    for (point, precision, accuracy) in [
        (point, SpatialPrecision::Premises, Some(1.5)),
        (
            GeoPoint::new(0.0, 0.0).unwrap(),
            SpatialPrecision::Representative,
            None,
        ),
    ] {
        let position = SpatialPosition::new(point, precision, accuracy).unwrap();
        let json = serde_json::to_string(&SpatialPositionData::from(position)).unwrap();
        assert_eq!(
            position,
            SpatialPosition::try_from(serde_json::from_str::<SpatialPositionData>(&json).unwrap())
                .unwrap()
        );
    }
    for unit in [
        DistanceUnit::Miles,
        DistanceUnit::Yards,
        DistanceUnit::Feet,
        DistanceUnit::Inches,
        DistanceUnit::Kilometers,
        DistanceUnit::Meters,
        DistanceUnit::Centimeters,
        DistanceUnit::Millimeters,
        DistanceUnit::NauticalMiles,
    ] {
        let distance = Distance::new(1.5, unit).unwrap();
        let json = serde_json::to_value(DistanceData::from(distance)).unwrap();
        assert_eq!(unit.as_str(), json["unit"]);
        assert_eq!(
            distance,
            Distance::try_from(serde_json::from_value::<DistanceData>(json).unwrap()).unwrap()
        );
    }
}

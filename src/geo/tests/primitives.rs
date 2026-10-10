use geo::{
    AddressText, CountryCode, DerivedGeography, Distance, DistanceUnit, GeoPoint,
    GeographicDescription, InvalidGeoText, PostalCode, SpatialPosition, SpatialPrecision,
    SubdivisionCode, SubmittedGeography, core::country::country_from_code,
    reference::ReferenceRelease,
};

#[test]
fn should_preserve_international_text_and_meaningful_line_breaks() {
    for source in [
        "  東京都千代田区\n丸の内１丁目９−１  ",
        "شارع المتنبي\r\nبغداد",
        "Straße 1\nŁódź\u{2028}España",
    ] {
        let text = AddressText::new(source).unwrap();
        assert_eq!(source, text.as_str());
    }
    for source in [
        "00123",
        "〒１００−０００１",
        "SW1A 1AA",
        "A-001/02",
        " 00123 ",
    ] {
        assert_eq!(source, PostalCode::new(source).unwrap().as_str());
    }
}

#[test]
fn should_reject_blank_control_and_oversized_text_without_truncation() {
    for blank in ["", " ", "\n\r\n", "\u{2003}"] {
        assert!(AddressText::new(blank).is_err());
        assert!(PostalCode::new(blank).is_err());
    }
    for control in ['\0', '\t', '\u{7f}', '\u{85}', '\u{1b}'] {
        let input = format!("a{control}b");
        assert_eq!(
            Err(InvalidGeoText::ControlCharacter),
            AddressText::new(&input)
        );
        assert_eq!(
            Err(InvalidGeoText::ControlCharacter),
            PostalCode::new(&input)
        );
    }
    assert!(AddressText::new("a\rb").is_err());
    for linebreak in ['\n', '\r', '\u{2028}', '\u{2029}'] {
        assert!(PostalCode::new(format!("a{linebreak}b")).is_err());
    }
    assert!(AddressText::new("x".repeat(AddressText::MAX_BYTES)).is_ok());
    assert_eq!(
        Err(InvalidGeoText::TooLong {
            max_bytes: AddressText::MAX_BYTES
        }),
        AddressText::new("x".repeat(AddressText::MAX_BYTES + 1))
    );
    assert!(AddressText::new("界".repeat(AddressText::MAX_BYTES / 3 + 1)).is_err());
    assert!(PostalCode::new("0".repeat(PostalCode::MAX_BYTES)).is_ok());
    assert!(PostalCode::new("0".repeat(PostalCode::MAX_BYTES + 1)).is_err());
}

#[test]
fn should_compare_postal_codes_conservatively_by_country() {
    let ca = PostalCode::new("k1a 0b1").unwrap();
    assert!(ca.equivalent_in(&PostalCode::new("K1A0B1").unwrap(), CountryCode::CAN));
    assert!(!ca.equivalent_in(&PostalCode::new("K1A0B1").unwrap(), CountryCode::USA));
    let gb = PostalCode::new("sw1a 1aa").unwrap();
    assert!(gb.equivalent_in(&PostalCode::new("SW1A1AA").unwrap(), CountryCode::GBR));
    for source in [
        "01234",
        "00123-0456",
        "not-known",
        "K 1 A 0 B 1",
        " K1A 0B1 ",
        "界000",
    ] {
        let postal = PostalCode::new(source).unwrap();
        for country in [
            CountryCode::DEU,
            CountryCode::JPN,
            CountryCode::CAN,
            CountryCode::GBR,
        ] {
            assert_eq!(source, postal.comparison_key(country));
        }
    }
    assert_ne!(
        PostalCode::new("00123")
            .unwrap()
            .comparison_key(CountryCode::USA),
        PostalCode::new("123")
            .unwrap()
            .comparison_key(CountryCode::USA)
    );
    assert_eq!("k1a 0b1", ca.as_str());
}

#[test]
fn should_support_partial_assertions_and_normalize_empty_to_absence() {
    assert_eq!(
        None,
        GeographicDescription::new(None, None, None, None).unwrap()
    );
    let country_only = GeographicDescription::new(None, Some(CountryCode::DEU), None, None)
        .unwrap()
        .unwrap();
    assert_eq!(Some(CountryCode::DEU), country_only.country());
    assert_eq!(ReferenceRelease::CURRENT, country_only.reference_release());
    assert_eq!(None, country_only.address_text());
    let subdivision = SubdivisionCode::new("DE-BE").unwrap();
    let description =
        GeographicDescription::new(None, Some(CountryCode::DEU), Some(subdivision), None)
            .unwrap()
            .unwrap();
    assert_eq!(Some(subdivision), description.subdivision());
    assert!(
        GeographicDescription::new(None, Some(CountryCode::USA), Some(subdivision), None).is_err()
    );
    let subdivision_only = GeographicDescription::new(None, None, Some(subdivision), None)
        .unwrap()
        .unwrap();
    assert_eq!(None, subdivision_only.country()); // Do not invent a source country assertion.
    assert_eq!(
        CountryCode::DEU,
        subdivision_only.subdivision().unwrap().country()
    );
    let postal_only =
        GeographicDescription::new(None, None, None, Some(PostalCode::new("00123").unwrap()))
            .unwrap();
    assert!(postal_only.is_some());
}

#[test]
fn should_preserve_unrecognized_postal_shapes_during_comparison() {
    for (country, source) in [
        (CountryCode::CAN, "d1a 0b1"),
        (CountryCode::CAN, "w1a 0b1"),
        (CountryCode::CAN, "z1a 0b1"),
        (CountryCode::CAN, "k1i 0b1"),
        (CountryCode::CAN, "k1a 0o1"),
        (CountryCode::GBR, "gir 1aa"),
        (CountryCode::GBR, "ai1 1aa"),
        (CountryCode::GBR, "az1 1aa"),
        (CountryCode::GBR, "s w1a1aa"),
        (CountryCode::GBR, "sw1a  1aa"),
    ] {
        let postal = PostalCode::new(source).unwrap();
        assert_eq!(source, postal.comparison_key(country));
        assert!(!postal.equivalent_in(
            &PostalCode::new(source.to_ascii_uppercase().replace(' ', "")).unwrap(),
            country
        ));
    }
    for (country, source, canonical) in [
        (CountryCode::CAN, "h0h 0h0", "H0H0H0"),
        (CountryCode::GBR, "gir 0aa", "GIR0AA"),
        (CountryCode::GBR, "m1 1aa", "M11AA"),
        (CountryCode::GBR, "m60 1nw", "M601NW"),
        (CountryCode::GBR, "cr2 6xh", "CR26XH"),
        (CountryCode::GBR, "dn55 1pt", "DN551PT"),
        (CountryCode::GBR, "w1a 1aa", "W1A1AA"),
        (CountryCode::GBR, "ec1a 1bb", "EC1A1BB"),
    ] {
        assert_eq!(
            canonical,
            PostalCode::new(source).unwrap().comparison_key(country)
        );
    }
}

#[test]
fn should_separate_submitted_assertions_and_derived_evidence() {
    let description = GeographicDescription::new(
        Some(AddressText::new("source text").unwrap()),
        None,
        None,
        None,
    )
    .unwrap()
    .unwrap();
    let submitted = SubmittedGeography::new(description);
    let derived = DerivedGeography::new(
        None,
        Some(
            SpatialPosition::new(
                GeoPoint::new(0.0, 0.0).unwrap(),
                SpatialPrecision::Representative,
                None,
            )
            .unwrap(),
        ),
    )
    .unwrap();
    assert_eq!(
        "source text",
        submitted.description().address_text().unwrap().as_str()
    );
    assert_eq!(None, derived.description());
    assert_eq!(None, DerivedGeography::new(None, None));
    assert!(derived.position().is_some());
}

#[test]
fn should_validate_every_coordinate_distance_and_accuracy_boundary() {
    for invalid in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        assert!(GeoPoint::new(invalid, 0.0).is_err());
        assert!(GeoPoint::new(0.0, invalid).is_err());
        assert!(Distance::new(invalid, DistanceUnit::Meters).is_err());
        assert!(
            SpatialPosition::new(
                GeoPoint::new(0.0, 0.0).unwrap(),
                SpatialPrecision::Premises,
                Some(invalid)
            )
            .is_err()
        );
    }
    for latitude in [-90.0001, 90.0001] {
        assert!(GeoPoint::new(latitude, 0.0).is_err());
    }
    for longitude in [-180.0001, 180.0001] {
        assert!(GeoPoint::new(0.0, longitude).is_err());
    }
    for (latitude, longitude) in [(-90.0, -180.0), (90.0, 180.0), (0.0, 0.0), (52.52, 13.405)] {
        let point = GeoPoint::new(latitude, longitude).unwrap();
        assert_eq!(latitude, point.latitude_degrees());
        assert_eq!(longitude, point.longitude_degrees());
    }
    assert!(Distance::new(-0.01, DistanceUnit::Meters).is_err());
    assert!(Distance::new(0.0, DistanceUnit::Meters).is_ok());
    let point = GeoPoint::new(52.0, 13.0).unwrap();
    assert!(SpatialPosition::new(point, SpatialPrecision::Street, Some(-1.0)).is_err());
    assert_eq!(
        None,
        SpatialPosition::new(point, SpatialPrecision::Locality, None)
            .unwrap()
            .accuracy_metres()
    );
    assert_eq!(
        Some(12.5),
        SpatialPosition::new(point, SpatialPrecision::Premises, Some(12.5))
            .unwrap()
            .accuracy_metres()
    );
    for precision in [
        SpatialPrecision::Premises,
        SpatialPrecision::Street,
        SpatialPrecision::Locality,
        SpatialPrecision::Representative,
    ] {
        assert_eq!(
            Some(precision),
            SpatialPrecision::from_code(precision.as_str())
        );
    }
    assert_eq!(None, SpatialPrecision::from_code("premises"));
}

#[test]
fn should_use_exact_reference_membership_and_preserve_historical_interpretation() {
    let release = ReferenceRelease::CURRENT;
    for code in ["de", " DE", "DEU", "UK", "XK", "EU", "ZZ", "AN", "界"] {
        assert!(country_from_code(code, release).is_err(), "{code}");
    }
    for code in ["de-be", "BE", "DE-XYZ", "US-XX", "EU-BE", "DE-BE ", "界"] {
        assert!(SubdivisionCode::new(code).is_err(), "{code}");
    }
    assert_eq!(
        Some(CountryCode::ALA),
        country_from_code("AX", release).ok()
    );
    assert!(ReferenceRelease::from_code("latest").is_err());
    assert!(ReferenceRelease::from_code("iso-codes-4.21.0").is_err());
    assert_eq!(
        Some("Netherlands Antilles"),
        release.former_country("ANHH").map(|row| row.name)
    );
    // AI is reused; historical French Afars and Issas must never become current Anguilla.
    assert_eq!(
        Some("French Afars and Issas"),
        release.former_country("AIDJ").map(|row| row.name)
    );
    assert_eq!(CountryCode::AIA, country_from_code("AI", release).unwrap());
    assert_eq!(None, release.former_country("AI"));
}

#[test]
fn should_cover_entire_vendored_release_and_existing_country_type() {
    let release = ReferenceRelease::CURRENT;
    let countries: serde_json::Value = serde_json::from_str(include_str!(
        "../reference/iso-codes-4.20.1/iso_3166-1.json"
    ))
    .unwrap();
    let subdivisions: serde_json::Value = serde_json::from_str(include_str!(
        "../reference/iso-codes-4.20.1/iso_3166-2.json"
    ))
    .unwrap();
    let former: serde_json::Value = serde_json::from_str(include_str!(
        "../reference/iso-codes-4.20.1/iso_3166-3.json"
    ))
    .unwrap();
    assert_eq!(249, release.country_codes().len());
    assert_eq!(CountryCode::iter().count(), release.country_codes().len());
    assert_eq!(
        countries["3166-1"].as_array().unwrap().len(),
        release.country_codes().len()
    );
    for row in countries["3166-1"].as_array().unwrap() {
        let code = row["alpha_2"].as_str().unwrap();
        assert_eq!(code, country_from_code(code, release).unwrap().alpha2());
    }
    assert_eq!(
        subdivisions["3166-2"].as_array().unwrap().len(),
        release.subdivision_codes().len()
    );
    assert!(release.subdivision_codes().len() > 5000);
    for row in subdivisions["3166-2"].as_array().unwrap() {
        let code = row["code"].as_str().unwrap();
        let subdivision = SubdivisionCode::from_code(code, release).unwrap();
        assert_eq!(code, subdivision.as_str());
        assert_eq!(&code[..2], subdivision.country().alpha2());
        assert_eq!(release, subdivision.reference_release());
        assert!(
            GeographicDescription::new(None, Some(subdivision.country()), Some(subdivision), None)
                .is_ok()
        );
    }
    for row in former["3166-3"].as_array().unwrap() {
        let country = release
            .former_country(row["alpha_4"].as_str().unwrap())
            .unwrap();
        assert_eq!(country.name, row["name"].as_str().unwrap());
        assert_eq!(country.former_alpha2, row["alpha_2"].as_str().unwrap());
        assert_eq!(
            country.withdrawal_date,
            row["withdrawal_date"].as_str().unwrap()
        );
    }
}

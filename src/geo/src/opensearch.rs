use crate::core::country::CountryCode;
use crate::core::{
    address::{InvalidStructuredAddress, StructuredAddress},
    distance::{Distance, DistanceUnit},
};

pub fn distance_to_opensearch_value(distance: Distance) -> String {
    format!(
        "{}{}",
        distance.amount(),
        distance_unit_suffix(distance.unit())
    )
}

fn distance_unit_suffix(unit: DistanceUnit) -> &'static str {
    match unit {
        DistanceUnit::Miles => "mi",
        DistanceUnit::Yards => "yd",
        DistanceUnit::Feet => "ft",
        DistanceUnit::Inches => "in",
        DistanceUnit::Kilometers => "km",
        DistanceUnit::Meters => "m",
        DistanceUnit::Centimeters => "cm",
        DistanceUnit::Millimeters => "mm",
        DistanceUnit::NauticalMiles => "nmi",
    }
}

pub fn structured_address_from_document(
    addressline: Option<String>,
    addressline_extra: Option<String>,
    locality: Option<String>,
    region: Option<String>,
    postal_code: Option<String>,
    country: Option<CountryCode>,
) -> Result<Option<StructuredAddress>, InvalidStructuredAddress> {
    let structured_address = StructuredAddress {
        addressline,
        addressline_extra,
        locality,
        region,
        postal_code,
        country,
    };
    structured_address.to_description()?;
    Ok((!structured_address.is_empty()).then_some(structured_address))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_reject_corrupt_documents_and_normalize_empty_to_absence() {
        assert_eq!(
            None,
            structured_address_from_document(None, None, None, None, None, None).unwrap()
        );
        assert!(
            structured_address_from_document(Some(" ".to_owned()), None, None, None, None, None)
                .is_err()
        );
        assert!(
            structured_address_from_document(
                None,
                None,
                None,
                None,
                Some("00123\0".to_owned()),
                None
            )
            .is_err()
        );
        let address = structured_address_from_document(
            None,
            None,
            None,
            None,
            Some("00123".to_owned()),
            Some(CountryCode::DEU),
        )
        .unwrap()
        .unwrap();
        assert_eq!(Some("00123"), address.postal_code.as_deref());
    }

    #[test]
    fn should_format_distance_for_opensearch() {
        assert_eq!(
            "50km",
            distance_to_opensearch_value(Distance::new(50.0, DistanceUnit::Kilometers).unwrap())
        );
        assert_eq!(
            "1.5nmi",
            distance_to_opensearch_value(Distance::new(1.5, DistanceUnit::NauticalMiles).unwrap())
        );
    }
}

//! The configured raw attribute registry: the stable camelCase keys under which a product schema
//! extracts product-specific values that have no normalized product field yet.

/// One raw attribute key a product schema may extract, with what it holds.
pub struct RawAttributeDefinition {
    pub key: &'static str,
    pub description: &'static str,
}

/// Raw attribute keys of one kind of product information, broad key first.
pub struct RawAttributeGroup {
    pub name: &'static str,
    pub definitions: &'static [RawAttributeDefinition],
}

const SHIPPING_RAW_ATTRIBUTES: &[RawAttributeDefinition] = &[
    RawAttributeDefinition {
        key: "rawShipment",
        description: "complete visible shipping, delivery, dispatch, pickup, or collection text",
    },
    RawAttributeDefinition {
        key: "rawShipmentNote",
        description: "shipping note, limitation, region, pickup-only text, or special delivery condition",
    },
];

const CONDITION_RAW_ATTRIBUTES: &[RawAttributeDefinition] = &[
    RawAttributeDefinition {
        key: "rawCondition",
        description: "complete visible condition text",
    },
    RawAttributeDefinition {
        key: "rawConditionNote",
        description: "condition note, caveat, defect, restoration, wear, or damage text",
    },
];

const MATERIAL_RAW_ATTRIBUTES: &[RawAttributeDefinition] = &[
    RawAttributeDefinition {
        key: "rawMaterial",
        description: "complete visible material or composition text",
    },
    RawAttributeDefinition {
        key: "rawMaterialNote",
        description: "material note, finish, technique, surface, or construction text",
    },
];

const YEAR_RAW_ATTRIBUTES: &[RawAttributeDefinition] = &[
    RawAttributeDefinition {
        key: "rawYear",
        description: "visible exact date, year, circa text, or date of manufacture",
    },
    RawAttributeDefinition {
        key: "rawPeriod",
        description: "visible period, era, century, or period category text",
    },
    RawAttributeDefinition {
        key: "rawYearNote",
        description: "date, period, era, or attribution note",
    },
];

const CATEGORY_RAW_ATTRIBUTES: &[RawAttributeDefinition] = &[
    RawAttributeDefinition {
        key: "rawCategory",
        description: "complete visible product category, type, or classification text",
    },
    RawAttributeDefinition {
        key: "rawCategoryPath",
        description: "visible breadcrumb or category path text",
    },
    RawAttributeDefinition {
        key: "rawTags",
        description: "visible product tags, labels, keywords, styles, or taxonomy terms",
    },
];

const MEASUREMENT_RAW_ATTRIBUTES: &[RawAttributeDefinition] = &[
    RawAttributeDefinition {
        key: "rawMeasurements",
        description: "complete visible dimensions, size, measurements, or weight text",
    },
    RawAttributeDefinition {
        key: "rawHeight",
        description: "visible height text",
    },
    RawAttributeDefinition {
        key: "rawWidth",
        description: "visible width text",
    },
    RawAttributeDefinition {
        key: "rawDepth",
        description: "visible depth text",
    },
    RawAttributeDefinition {
        key: "rawDiameter",
        description: "visible diameter text",
    },
    RawAttributeDefinition {
        key: "rawWeight",
        description: "visible weight text",
    },
    RawAttributeDefinition {
        key: "rawMeasurementNote",
        description: "measurement note, unit note, approximation, or size caveat text",
    },
];

const ORIGIN_RAW_ATTRIBUTES: &[RawAttributeDefinition] = &[
    RawAttributeDefinition {
        key: "rawOrigin",
        description: "complete visible origin, provenance, maker location, or place text",
    },
    RawAttributeDefinition {
        key: "rawCountry",
        description: "visible country text",
    },
    RawAttributeDefinition {
        key: "rawRegion",
        description: "visible region, city, locality, or area text",
    },
    RawAttributeDefinition {
        key: "rawOriginNote",
        description: "origin, provenance, attribution, or locality note",
    },
];

const CREATOR_RAW_ATTRIBUTES: &[RawAttributeDefinition] = &[
    RawAttributeDefinition {
        key: "rawArtistName",
        description: "visible artist or creator name",
    },
    RawAttributeDefinition {
        key: "rawMakerName",
        description: "visible maker, manufacturer, or workshop name",
    },
    RawAttributeDefinition {
        key: "rawDesignerName",
        description: "visible designer name",
    },
    RawAttributeDefinition {
        key: "rawBrandName",
        description: "visible product-specific brand or label name",
    },
    RawAttributeDefinition {
        key: "rawSignature",
        description: "visible signed, unsigned, signature, or maker mark text",
    },
    RawAttributeDefinition {
        key: "rawCreatorNote",
        description: "visible attribution, follower-of, school-of, studio, workshop, or authorship note",
    },
];

/// Every raw attribute key a product schema extracts, grouped by kind.
pub const RAW_ATTRIBUTE_GROUPS: &[RawAttributeGroup] = &[
    RawAttributeGroup {
        name: "shipping",
        definitions: SHIPPING_RAW_ATTRIBUTES,
    },
    RawAttributeGroup {
        name: "condition",
        definitions: CONDITION_RAW_ATTRIBUTES,
    },
    RawAttributeGroup {
        name: "material",
        definitions: MATERIAL_RAW_ATTRIBUTES,
    },
    RawAttributeGroup {
        name: "year",
        definitions: YEAR_RAW_ATTRIBUTES,
    },
    RawAttributeGroup {
        name: "category",
        definitions: CATEGORY_RAW_ATTRIBUTES,
    },
    RawAttributeGroup {
        name: "measurements",
        definitions: MEASUREMENT_RAW_ATTRIBUTES,
    },
    RawAttributeGroup {
        name: "origin",
        definitions: ORIGIN_RAW_ATTRIBUTES,
    },
    RawAttributeGroup {
        name: "creator",
        definitions: CREATOR_RAW_ATTRIBUTES,
    },
];

/// Each registered definition with the name of its group, in registry order.
pub fn registered_definitions()
-> impl Iterator<Item = (&'static str, &'static RawAttributeDefinition)> {
    RAW_ATTRIBUTE_GROUPS.iter().flat_map(|group| {
        group
            .definitions
            .iter()
            .map(move |definition| (group.name, definition))
    })
}

pub fn is_registered_key(key: &str) -> bool {
    registered_definitions().any(|(_, definition)| definition.key == key)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn should_register_each_key_once_as_a_camel_case_raw_name() {
        let keys: Vec<&str> = registered_definitions()
            .map(|(_, definition)| definition.key)
            .collect();

        assert_eq!(keys.iter().collect::<HashSet<_>>().len(), keys.len());
        assert!(keys.iter().all(|key| {
            key.strip_prefix("raw").is_some_and(|name| {
                name.starts_with(|first: char| first.is_ascii_uppercase())
                    && name
                        .chars()
                        .all(|character| character.is_ascii_alphanumeric())
            })
        }));
    }

    #[test]
    fn should_recognize_only_registered_keys() {
        assert!(is_registered_key("rawShipment"));
        assert!(is_registered_key("rawCreatorNote"));
        assert!(!is_registered_key("rawshipment"));
        assert!(!is_registered_key("rawFoo"));
        assert!(!is_registered_key(""));
    }
}

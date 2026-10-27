use serde_json::{Value, json};

#[test]
fn should_constrain_documented_shopify_identity_to_exact_unsigned_64_bit_values() {
    let spec: Value = yaml_serde::from_str(include_str!("../../../docs/swagger.yaml")).unwrap();
    let validator =
        jsonschema::validator_for(&spec["components"]["schemas"]["ShopifyProductIdentity"])
            .unwrap();
    for (value, accepted) in [
        (json!(1), true),
        (json!(9_007_199_254_740_993_u64), true),
        (json!(u64::MAX), true),
        (json!("9007199254740993"), true),
        (json!("18446744073709551615"), true),
        (json!("18446744073709551616"), false),
        (json!("99999999999999999999"), false),
        (json!("042"), false),
        (json!("0"), false),
        (json!(0), false),
        (json!(-1), false),
        (json!(1.5), false),
    ] {
        assert_eq!(accepted, validator.is_valid(&value), "{value}");
    }
}

use super::*;

#[tokio::test]
async fn should_match_advertised_input_schemas_for_required_optional_and_nullable_fields() {
    let fixture = Fixture::new(Outcome::Success);
    let registry = registry(&fixture);
    let user = UserId::new();
    let product = fixture.product.item.product_listing_id;
    let cases = [
        (
            "getWatchlistProductListings",
            vec![
                (json!({}), true),
                (
                    json!({"language":"en_IE","currency":"USD","size":u64::MAX}),
                    true,
                ),
                (json!({"size":null,"searchAfter":null}), true),
                (json!({"language":null}), false),
                (json!({"currency":null}), false),
                (json!({"language":"xx"}), false),
                (json!({"currency":"XXX"}), false),
                (json!({"size":-1}), false),
                (json!({"size":18446744073709551616_f64}), false),
                (json!({"size":1.5}), false),
                (json!({"size":"1"}), false),
                (json!({"searchAfter":[]}), false),
                (json!({"userId":user}), false),
            ],
        ),
        (
            "addWatchlistProduct",
            vec![
                (json!({"productListingId":product}), true),
                (
                    json!({"productListingId":product,"notifications":null}),
                    true,
                ),
                (
                    json!({"productListingId":product,"notifications":false}),
                    true,
                ),
                (json!({}), false),
                (json!({"productListingId":null}), false),
                (json!({"productListingId":user}), false),
                (json!({"productListingId":product.as_uuid()}), false),
                (
                    json!({"productListingId":product,"notifications":"true"}),
                    false,
                ),
                (json!({"productListingId":product,"userId":user}), false),
            ],
        ),
        (
            "deleteWatchlistProduct",
            vec![
                (json!({"productListingId":product}), true),
                (json!({}), false),
                (json!({"productListingId":null}), false),
                (json!({"productListingId":user}), false),
                (json!({"productListingId":product.as_uuid()}), false),
                (json!({"productListingId":product,"userId":user}), false),
            ],
        ),
        (
            "patchWatchlistProduct",
            vec![
                (json!({"productListingId":product}), true),
                (
                    json!({"productListingId":product,"notifications":false,"state":"INACTIVE_BY_USER","language":"de-CH","currency":"USD"}),
                    true,
                ),
                (json!({"productListingId":product,"state":"ACTIVE"}), true),
                (json!({}), false),
                (json!({"productListingId":null}), false),
                (
                    json!({"productListingId":product,"notifications":null}),
                    false,
                ),
                (json!({"productListingId":product,"state":null}), false),
                (
                    json!({"productListingId":product,"state":"INACTIVE_BY_RESTRICTED_PLAN"}),
                    false,
                ),
                (json!({"productListingId":product,"language":null}), false),
                (json!({"productListingId":product,"currency":null}), false),
                (json!({"productListingId":product,"userId":user}), false),
            ],
        ),
    ];
    for (name, cases) in cases {
        let schema =
            serde_json::to_value(&registry.get(name).unwrap().tool().input_schema).unwrap();
        for (args, accepted) in cases {
            assert_eq!(
                accepted,
                jsonschema::is_valid(&schema, &args),
                "{name}: {args}"
            );
            let result = call(&registry, name, args.clone(), user).await;
            match result {
                Ok(value) => {
                    assert!(accepted, "{name}: {args}");
                    let output_schema = serde_json::to_value(
                        registry
                            .get(name)
                            .unwrap()
                            .tool()
                            .output_schema
                            .as_ref()
                            .unwrap(),
                    )
                    .unwrap();
                    assert!(
                        jsonschema::is_valid(&output_schema, &value),
                        "{name}: {value}"
                    );
                }
                Err(OperationError::InvalidArguments(_)) => assert!(!accepted, "{name}: {args}"),
                Err(error) => panic!("unexpected {name} error for {args}: {error:?}"),
            }
        }
    }
}

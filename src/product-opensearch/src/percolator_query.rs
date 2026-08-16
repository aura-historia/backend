use crate::product_document::ProductDocumentSerdeField;
use crate::product_search_reader::build_filter_clauses;
use common::language::domain::Language;
use common::query::text_query::TextQuery;
use product_service::ports::CompiledProductSearch;
use serde_json::json;

/// Builds stable percolator JSON for one complete Product search compiled against a pinned FX snapshot.
///
/// This returns only OpenSearch JSON. Product documents remain adapter-private.
pub fn build_percolator_query(
    compiled_search: &CompiledProductSearch,
) -> Result<serde_json::Value, serde_json::Error> {
    let search = &compiled_search.search;
    let mut must = Vec::with_capacity(1);
    if let Some(product_query_clause) =
        build_product_query_clause(&search.product_query, title_field(&search.language))
    {
        must.push(product_query_clause);
    }
    let (must_not, filter) = build_filter_clauses(search, &compiled_search.price_filter_plan)?;

    Ok(json!({
        "bool": {
            "must": must,
            "must_not": must_not,
            "filter": filter
        }
    }))
}

fn title_field(language: &Language) -> ProductDocumentSerdeField {
    match language {
        Language::De => ProductDocumentSerdeField::TitleDe,
        Language::En => ProductDocumentSerdeField::TitleEn,
        Language::Fr => ProductDocumentSerdeField::TitleFr,
        Language::Es => ProductDocumentSerdeField::TitleEs,
        Language::It => ProductDocumentSerdeField::TitleIt,
        _ => ProductDocumentSerdeField::TitleEn,
    }
}

fn build_product_query_clause(
    product_queries: &[TextQuery<1>],
    title_field: ProductDocumentSerdeField,
) -> Option<serde_json::Value> {
    match product_queries {
        [] => None,
        [product_query] => Some(build_text_match_clause(product_query.as_ref(), title_field)),
        product_queries => Some(json!({
            "bool": {
                "should": product_queries
                    .iter()
                    .map(|product_query| build_text_match_clause(product_query.as_ref(), title_field))
                    .collect::<Vec<_>>(),
                "minimum_should_match": 1
            }
        })),
    }
}

fn build_text_match_clause(
    product_query: &str,
    title_field: ProductDocumentSerdeField,
) -> serde_json::Value {
    json!({
        "multi_match": {
            "query": product_query,
            "fields": [title_field.as_str(), "title.text"],
            "type": "best_fields",
            "operator": "or",
            "minimum_should_match": "4<80%"
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use common::{
        currency::domain::Currency, fx_rate_id::FxRateId, price::domain::MonetaryAmount,
        query::range_query::RangeQuery,
    };
    use fxrate_core::{FX_RATE_SCALE, FxRateQuote, FxRateSource, NewFxRateSnapshot};
    use product_service::ports::ProductPriceFilterPlan;
    use strum::IntoEnumIterator;
    use time::OffsetDateTime;

    fn price_filter() -> Result<ProductPriceFilterPlan, Box<dyn std::error::Error>> {
        let snapshot = NewFxRateSnapshot::capture_eur(
            FxRateId::new(),
            OffsetDateTime::UNIX_EPOCH,
            FxRateSource::FxRatesApi,
            Currency::Eur,
            Currency::iter().map(|currency| {
                FxRateQuote::new(
                    currency,
                    if currency == Currency::Usd {
                        1_100_000
                    } else {
                        FX_RATE_SCALE
                    },
                )
            }),
        )?
        .into_persisted(1_i64.try_into()?);
        Ok(ProductPriceFilterPlan::compile(
            snapshot,
            Currency::Usd,
            Some(RangeQuery {
                min: Some(MonetaryAmount::from(110_u64)),
                max: Some(MonetaryAmount::from(110_u64)),
            }),
        )?)
    }

    #[test]
    fn should_build_percolator_price_filter_from_pinned_plan()
    -> Result<(), Box<dyn std::error::Error>> {
        let query = build_percolator_query(&CompiledProductSearch {
            search: product_core::product_search::ProductSearch::new(Language::En, Currency::Usd),
            price_filter_plan: price_filter()?,
        })?;

        assert_eq!(
            query.pointer("/bool/filter/1/bool/should/0/bool/must_not/0/exists/field"),
            Some(&json!("saleFxRateId"))
        );
        assert_eq!(
            query.pointer("/bool/filter/1/bool/should/0/bool/filter/0/bool/should/0/bool/filter/0/term/sourcePrice.currency"),
            Some(&json!("EUR"))
        );
        assert_eq!(
            query.pointer("/bool/filter/1/bool/should/0/bool/filter/0/bool/should/0/bool/filter/1/range/sourcePrice.amount/gte"),
            Some(&json!(100))
        );
        assert_eq!(
            query.pointer("/bool/filter/1/bool/should/1/bool/filter/0/exists/field"),
            Some(&json!("saleFxRateId"))
        );
        assert_eq!(
            query.pointer("/bool/filter/1/bool/should/1/bool/filter/1/range/salePrices.usd/gte"),
            Some(&json!(110))
        );
        Ok(())
    }
}

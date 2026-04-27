use url::Url;

pub fn append_utm_params(mut url: Url) -> Url {
    url.query_pairs_mut()
        .append_pair("utm_source", "aura_historia")
        .append_pair("utm_medium", "referral");
    url
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_append_utm_params_when_url_has_no_query_params() {
        let url = Url::parse("https://example.com/product/123").unwrap();
        let result = append_utm_params(url);
        assert_eq!(
            result.as_str(),
            "https://example.com/product/123?utm_source=aura_historia&utm_medium=referral"
        );
    }

    #[test]
    fn should_append_utm_params_when_url_already_has_query_params() {
        let url = Url::parse("https://example.com/product/123?color=red").unwrap();
        let result = append_utm_params(url);
        assert_eq!(
            result.as_str(),
            "https://example.com/product/123?color=red&utm_source=aura_historia&utm_medium=referral"
        );
    }

    #[test]
    fn should_set_utm_source_to_aura_historia_when_appending_params() {
        let url = Url::parse("https://shop.test/item/42").unwrap();
        let result = append_utm_params(url);
        let pairs: Vec<(_, _)> = result.query_pairs().collect();
        assert!(
            pairs
                .iter()
                .any(|(k, v)| k == "utm_source" && v == "aura_historia")
        );
    }

    #[test]
    fn should_set_utm_medium_to_referral_when_appending_params() {
        let url = Url::parse("https://shop.test/item/42").unwrap();
        let result = append_utm_params(url);
        let pairs: Vec<(_, _)> = result.query_pairs().collect();
        assert!(
            pairs
                .iter()
                .any(|(k, v)| k == "utm_medium" && v == "referral")
        );
    }
}

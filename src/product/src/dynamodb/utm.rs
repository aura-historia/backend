use url::Url;

pub fn append_utm_params(mut url: Url) -> Url {
    let already_has_utm = url.query_pairs().any(|(key, _)| key == "utm_source");
    if already_has_utm {
        return url;
    }
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
    fn should_not_duplicate_utm_params_when_url_already_has_utm() {
        let url = Url::parse(
            "https://example.com/product/123?utm_source=aura_historia&utm_medium=referral",
        )
        .unwrap();
        let result = append_utm_params(url);
        assert_eq!(
            result.as_str(),
            "https://example.com/product/123?utm_source=aura_historia&utm_medium=referral"
        );
    }
}

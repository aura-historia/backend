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

/// Strips `utm_source` and `utm_medium` query parameters from a URL, returning
/// the canonical URL without UTM tracking parameters.
///
/// Used internally before URL-equality comparisons so that a URL that was
/// enriched with UTM params during a persistence → domain mapping still
/// compares equal to its un-enriched counterpart coming from an inbound command.
pub fn strip_utm_params(url: Url) -> Url {
    let filtered: Vec<(String, String)> = url
        .query_pairs()
        .filter(|(key, _)| key != "utm_source" && key != "utm_medium")
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect();
    let mut stripped = url;
    if filtered.is_empty() {
        stripped.set_query(None);
    } else {
        stripped.query_pairs_mut().clear().extend_pairs(filtered);
    }
    stripped
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_append_utm_params_when_url_has_no_query_params() {
        let url = Url::parse("https://example.com/item").unwrap();
        let result = append_utm_params(url);
        assert_eq!(
            result.as_str(),
            "https://example.com/item?utm_source=aura_historia&utm_medium=referral"
        );
    }

    #[test]
    fn should_append_utm_params_when_url_already_has_query_params() {
        let url = Url::parse("https://example.com/item?ref=homepage").unwrap();
        let result = append_utm_params(url);
        assert_eq!(
            result.as_str(),
            "https://example.com/item?ref=homepage&utm_source=aura_historia&utm_medium=referral"
        );
    }

    #[test]
    fn should_not_duplicate_utm_params_when_url_already_has_utm() {
        let url =
            Url::parse("https://example.com/item?utm_source=aura_historia&utm_medium=referral")
                .unwrap();
        let result = append_utm_params(url);
        assert_eq!(
            result.as_str(),
            "https://example.com/item?utm_source=aura_historia&utm_medium=referral"
        );
    }

    #[test]
    fn should_strip_utm_params_when_url_has_only_utm_params() {
        let url =
            Url::parse("https://example.com/item?utm_source=aura_historia&utm_medium=referral")
                .unwrap();
        let result = strip_utm_params(url);
        assert_eq!(result.as_str(), "https://example.com/item");
    }

    #[test]
    fn should_preserve_non_utm_params_when_stripping_utm_params() {
        let url = Url::parse(
            "https://example.com/item?ref=homepage&utm_source=aura_historia&utm_medium=referral",
        )
        .unwrap();
        let result = strip_utm_params(url);
        assert_eq!(result.as_str(), "https://example.com/item?ref=homepage");
    }

    #[test]
    fn should_be_identity_when_no_utm_params_present() {
        let url = Url::parse("https://example.com/item?ref=homepage").unwrap();
        let result = strip_utm_params(url);
        assert_eq!(result.as_str(), "https://example.com/item?ref=homepage");
    }
}

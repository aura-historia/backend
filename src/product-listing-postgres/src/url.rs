use url::Url;

pub(crate) fn append_utm_params(mut url: Url) -> Url {
    if url.query_pairs().any(|(key, _)| key == "utm_source") {
        return url;
    }

    url.query_pairs_mut()
        .append_pair("utm_source", "aura_historia")
        .append_pair("utm_medium", "referral");
    url
}

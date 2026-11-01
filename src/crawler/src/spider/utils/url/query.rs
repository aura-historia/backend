/// Drops `utm_*` campaign parameters and keeps every other parameter exactly as fetched. A
/// query left empty is dropped with its `?`.
pub(super) fn drop_tracking_parameters(url: &mut url::Url) {
    let Some(query) = url.query() else {
        return;
    };
    let kept = query
        .split('&')
        .filter(|pair| !is_tracking_parameter(pair))
        .collect::<Vec<_>>()
        .join("&");
    if kept.is_empty() {
        url.set_query(None);
    } else if kept != query {
        url.set_query(Some(&kept));
    }
}

fn is_tracking_parameter(pair: &str) -> bool {
    url::form_urlencoded::parse(pair.as_bytes())
        .next()
        .is_some_and(|(key, _)| {
            key.get(..4)
                .is_some_and(|prefix| prefix.eq_ignore_ascii_case("utm_"))
        })
}

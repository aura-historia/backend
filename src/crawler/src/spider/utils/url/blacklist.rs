use spider::compact_str::CompactString;

const BLACKLIST_URL_SUBSTRINGS: &[&str] = &[
    "cart",
    "wishlist",
    "?replytocom=",
    "&replytocom=",
    "/wp-admin/",
    ".jpg",
    ".jpeg",
    ".png",
    ".webp",
    ".gif",
    ".svg",
    ".ico",
    ".pdf",
    ".zip",
    ".csv",
    ".xml",
    ".json",
    "/download",
    "downloadproductimages",
    "/file",
    "/files",
    "/attachment",
    "/attachments",
];

pub(super) fn is_blacklisted(url: &str) -> bool {
    BLACKLIST_URL_SUBSTRINGS
        .iter()
        .any(|pattern| url.contains(pattern))
}

pub(super) fn patterns() -> Vec<CompactString> {
    BLACKLIST_URL_SUBSTRINGS
        .iter()
        .map(|pattern| CompactString::from(regex::escape(pattern)))
        .collect()
}

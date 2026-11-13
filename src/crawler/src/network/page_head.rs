//! The `<head>` of a fetched page, the part of it page-level metadata is read from: the robots
//! directives of the scraper and the identity links of the spider.

/// How far into a page the `<head>` is looked for. Shopify heads run to about 60 KiB; a page
/// whose head ends later is read up to this bound only.
pub const MAX_HEAD_BYTES: usize = 512 * 1024;

const HEAD_END: &[u8] = b"</head";

/// The page up to the end of its `<head>`, at most [`MAX_HEAD_BYTES`]; that bound of the page
/// when no end tag is found within it.
pub fn head_of(html: &[u8]) -> &[u8] {
    let bounded = &html[..html.len().min(MAX_HEAD_BYTES)];
    bounded
        .windows(HEAD_END.len())
        .position(|window| window.eq_ignore_ascii_case(HEAD_END))
        .map_or(bounded, |end| &bounded[..end])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_end_the_head_at_its_end_tag_in_any_case() {
        assert_eq!(
            head_of(b"<html><head><title>x</title></HEAD><body>y</body>"),
            b"<html><head><title>x</title>"
        );
    }

    #[test]
    fn should_bound_a_page_without_a_head_end_tag() {
        let page = vec![b'a'; MAX_HEAD_BYTES + 10];

        assert_eq!(head_of(&page).len(), MAX_HEAD_BYTES);
    }
}

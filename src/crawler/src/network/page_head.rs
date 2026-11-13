//! The `<head>` of a fetched page, the part of it page-level metadata is read from: the robots
//! directives of the scraper and the identity links of the spider.

/// How far into a page the `<head>` is looked for. Shopify heads run to about 60 KiB; a page
/// whose head ends later is read up to this bound only.
pub const MAX_HEAD_BYTES: usize = 512 * 1024;

const HEAD_END: &[u8] = b"</head";

/// The page up to the end of its `<head>`, at most [`MAX_HEAD_BYTES`]; that bound of the page
/// when no end tag is found within it. The end tag's name must end where a tag name ends, so
/// `</header>` in the head's text does not end it.
pub fn head_of(html: &[u8]) -> &[u8] {
    let bounded = &html[..html.len().min(MAX_HEAD_BYTES)];
    let ends_tag_name = |byte: Option<&u8>| {
        byte.is_none_or(|byte| *byte == b'>' || *byte == b'/' || byte.is_ascii_whitespace())
    };
    (0..bounded.len().saturating_sub(HEAD_END.len() - 1))
        .find(|&start| {
            bounded[start..start + HEAD_END.len()].eq_ignore_ascii_case(HEAD_END)
                && ends_tag_name(bounded.get(start + HEAD_END.len()))
        })
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
    fn should_not_end_the_head_at_a_longer_tag_name() {
        let page =
            b"<head><script>t='</header>'</script><meta name=robots content=noindex></head ><body>";

        assert_eq!(
            head_of(page),
            b"<head><script>t='</header>'</script><meta name=robots content=noindex>"
        );
    }

    #[test]
    fn should_bound_a_page_without_a_head_end_tag() {
        let page = vec![b'a'; MAX_HEAD_BYTES + 10];

        assert_eq!(head_of(&page).len(), MAX_HEAD_BYTES);
    }
}

//! The `srcset` candidate grammar, read the same way by image extraction and by the schema
//! generation projection, so both see the same candidates of a page's `srcset`.

/// The candidates of a `srcset`, each as its URL and its descriptors, such as ` 2x` or ` 800w`,
/// as the HTML standard's "parse a srcset attribute" algorithm splits them: a URL runs to the
/// next ASCII whitespace, so a URL whose path or query holds commas, such as
/// `/w_400,h_300/photo.jpg` or a `data:` URI's payload, stays whole, and only trailing commas
/// end it early. Its descriptors then run to the next comma outside parentheses. Leading
/// whitespace and commas between candidates are skipped, so `a.jpg 1x,, ,b.jpg 2x` holds two
/// candidates while `a.jpg,b.jpg`, with no whitespace, is one URL, as browsers read it.
pub(crate) fn srcset_candidates(srcset: &str) -> impl Iterator<Item = (&str, &str)> {
    let mut rest = srcset;
    std::iter::from_fn(move || {
        rest = rest.trim_start_matches(|c: char| c.is_ascii_whitespace() || c == ',');
        if rest.is_empty() {
            return None;
        }
        let (raw_url, after_url) = rest.split_at(
            rest.find(|c: char| c.is_ascii_whitespace())
                .unwrap_or(rest.len()),
        );
        let url = raw_url.trim_end_matches(',');
        if url.len() < raw_url.len() {
            // Trailing commas closed the candidate, so no descriptors follow its URL.
            rest = after_url;
            return Some((url, ""));
        }
        let (descriptors, after_candidate) = after_url.split_at(descriptors_end(after_url));
        rest = after_candidate;
        Some((url, descriptors))
    })
}

/// Where the descriptors at the start of `after_url` end: at the first comma outside
/// parentheses, or at the end of the `srcset`.
fn descriptors_end(after_url: &str) -> usize {
    let mut in_parentheses = false;
    for (index, c) in after_url.char_indices() {
        match c {
            '(' => in_parentheses = true,
            ')' => in_parentheses = false,
            ',' if !in_parentheses => return index,
            _ => {}
        }
    }
    after_url.len()
}

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::rstest;

    #[rstest]
    #[case::urls_and_descriptors("a.jpg 1x, b.jpg 2x", &[("a.jpg", " 1x"), ("b.jpg", " 2x")])]
    #[case::comma_without_whitespace_inside_one_url("a.jpg,b.jpg", &[("a.jpg,b.jpg", "")])]
    #[case::single_url(" /uploads/photo.jpg ", &[("/uploads/photo.jpg", " ")])]
    #[case::cdn_transformations_with_commas(
        "https://res.example.com/w_400,h_300,c_fill/photo.jpg 400w, https://res.example.com/w_800,h_600,c_fill/photo.jpg 800w",
        &[
            ("https://res.example.com/w_400,h_300,c_fill/photo.jpg", " 400w"),
            ("https://res.example.com/w_800,h_600,c_fill/photo.jpg", " 800w"),
        ]
    )]
    #[case::query_with_commas("/photo.jpg?crop=0,0,800,600 800w", &[("/photo.jpg?crop=0,0,800,600", " 800w")])]
    #[case::trailing_comma_ends_the_url("/a.jpg,, /b.jpg 2x", &[("/a.jpg", ""), ("/b.jpg", " 2x")])]
    #[case::data_uri_with_descriptor(
        "data:image/gif;base64,R0lG 1x, /a.jpg 2x",
        &[("data:image/gif;base64,R0lG", " 1x"), ("/a.jpg", " 2x")]
    )]
    #[case::data_uri_closed_by_a_comma(
        "data:image/gif;base64,R0lG,, /a.jpg",
        &[("data:image/gif;base64,R0lG", ""), ("/a.jpg", "")]
    )]
    #[case::comma_inside_parentheses_of_a_descriptor(
        "/a.jpg 1x (x, y), /b.jpg 2x",
        &[("/a.jpg", " 1x (x, y)"), ("/b.jpg", " 2x")]
    )]
    #[case::empty(" , ", &[])]
    fn should_split_a_srcset_into_its_candidates_as_browsers_do(
        #[case] srcset: &str,
        #[case] expected: &[(&str, &str)],
    ) {
        assert_eq!(srcset_candidates(srcset).collect::<Vec<_>>(), expected);
    }
}

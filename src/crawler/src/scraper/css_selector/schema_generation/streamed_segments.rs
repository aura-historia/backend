//! React's streamed server rendering, which Next.js shops use, sends a page's slow parts after its
//! shell: each part arrives as a hidden segment, `<div hidden id="S:…">`, at the end of the body,
//! with an inline script call that moves it into the place it fills. `$RC("B:…","S:…")` (or
//! `$RR`, the same with stylesheets) replaces a Suspense boundary's fallback with the segment, and
//! `$RS("S:…","P:…")` replaces a placeholder `<template>`. A browser runs those calls before anyone
//! reads the page; the projection runs no script, so without this step a streamed product page's
//! content would sit hidden at its end. The projection therefore makes the same moves before it
//! removes the scripts and the comments that delimit the boundaries. A segment no call names
//! stays where it is, hidden, as a browser leaves it.

use kuchiki::NodeRef;

/// The most streamed segment moves one page gets, the first in script order; a real page streams
/// a few dozen at most.
const MAX_SEGMENT_MOVES: usize = 500;

/// One script call that moves a streamed segment into the page.
#[derive(Debug, PartialEq, Eq)]
enum SegmentMove {
    /// `$RC` or `$RR`: the segment replaces the Suspense boundary's content, its fallback.
    CompleteBoundary { boundary: String, segment: String },
    /// `$RS`: the segment replaces the placeholder template.
    CompleteSegment {
        segment: String,
        placeholder: String,
    },
}

/// Moves every streamed segment of `document` that a script call names into its place, in the
/// order of the calls, as React's streaming runtime does in a browser.
pub(super) fn place_streamed_segments(document: &NodeRef) {
    for segment_move in segment_moves(document) {
        match segment_move {
            SegmentMove::CompleteBoundary { boundary, segment } => {
                complete_boundary(document, &boundary, &segment)
            }
            SegmentMove::CompleteSegment {
                segment,
                placeholder,
            } => complete_segment(document, &segment, &placeholder),
        }
    }
}

/// The segment moves the inline scripts of `document` call, in document order.
fn segment_moves(document: &NodeRef) -> Vec<SegmentMove> {
    let call = regex::regex!(r#"\$R([CRS])\("([^"\\]{1,64})","([^"\\]{1,64})""#);
    let Ok(scripts) = document.select("script:not([src])") else {
        return Vec::new();
    };
    scripts
        .flat_map(|script| {
            let text = script.text_contents();
            call.captures_iter(&text)
                .map(|captures| {
                    let (first, second) = (captures[2].to_owned(), captures[3].to_owned());
                    if &captures[1] == "S" {
                        SegmentMove::CompleteSegment {
                            segment: first,
                            placeholder: second,
                        }
                    } else {
                        SegmentMove::CompleteBoundary {
                            boundary: first,
                            segment: second,
                        }
                    }
                })
                .collect::<Vec<_>>()
        })
        .take(MAX_SEGMENT_MOVES)
        .collect()
}

/// `$RC`: removes the boundary's content, from its `<template id="B:…">` up to the comment that
/// closes the boundary, and puts the segment's children there. A boundary that is gone leaves the
/// segment where it is.
fn complete_boundary(document: &NodeRef, boundary_id: &str, segment_id: &str) {
    let (Some(boundary), Some(segment)) = (
        element_by_id(document, boundary_id),
        element_by_id(document, segment_id),
    ) else {
        return;
    };
    let Some(parent) = boundary.parent() else {
        return;
    };
    let boundary_end = remove_boundary_content(&boundary);
    segment.detach();
    for child in segment.children().collect::<Vec<_>>() {
        match &boundary_end {
            Some(end) => end.insert_before(child),
            None => parent.append(child),
        }
    }
}

/// Removes `start` and its following siblings up to the comment that closes the boundary it opens,
/// and returns that comment, or `None` when the boundary is not closed in its parent. Nested
/// boundaries (`<!--$-->`, `<!--$?-->`, `<!--$!-->`) are removed whole.
fn remove_boundary_content(start: &NodeRef) -> Option<NodeRef> {
    let mut depth = 0usize;
    let mut next = Some(start.clone());
    while let Some(node) = next {
        let marker = node.as_comment().map(|comment| comment.borrow().clone());
        match marker.as_deref() {
            Some("/$") if depth == 0 => return Some(node),
            Some("/$") => depth -= 1,
            Some("$" | "$?" | "$!") => depth += 1,
            _ => {}
        }
        next = node.next_sibling();
        node.detach();
    }
    None
}

/// `$RS`: puts the segment's children in the placeholder template's place.
fn complete_segment(document: &NodeRef, segment_id: &str, placeholder_id: &str) {
    let (Some(segment), Some(placeholder)) = (
        element_by_id(document, segment_id),
        element_by_id(document, placeholder_id),
    ) else {
        return;
    };
    segment.detach();
    for child in segment.children().collect::<Vec<_>>() {
        placeholder.insert_before(child);
    }
    placeholder.detach();
}

fn element_by_id(document: &NodeRef, id: &str) -> Option<NodeRef> {
    document.descendants().find(|node| {
        node.as_element()
            .is_some_and(|element| element.attributes.borrow().get("id") == Some(id))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use kuchiki::parse_html;
    use kuchiki::traits::*;

    fn placed(html: &str) -> String {
        let document = parse_html().one(html);
        place_streamed_segments(&document);
        document
            .select_first("body")
            .expect("body")
            .as_node()
            .children()
            .filter(|node| {
                node.as_element()
                    .is_none_or(|element| &*element.name.local != "script")
            })
            .map(|node| node.to_string())
            .collect()
    }

    /// The Drew Pritchard product page (#3008): its price and buy button arrive in streamed
    /// segments that replace a loading fallback and a placeholder.
    #[test]
    fn should_move_streamed_segments_into_the_boundary_and_the_placeholder_they_fill() {
        let html = concat!(
            r#"<body><main><h1>Country Table</h1><!--$?--><template id="B:0"></template>"#,
            r#"<p>Loading…</p><!--/$--><template id="P:1"></template></main>"#,
            r#"<div hidden id="S:0"><p>£1,200</p><button>Buy now</button></div>"#,
            r#"<script>$RC("B:0","S:0")</script>"#,
            r#"<div hidden id="S:1"><p>Free delivery</p></div>"#,
            r#"<script>$RS("S:1","P:1")</script></body>"#,
        );

        assert_eq!(
            concat!(
                "<main><h1>Country Table</h1><!--$?--><p>£1,200</p><button>Buy now</button>",
                "<!--/$--><p>Free delivery</p></main>"
            ),
            placed(html)
        );
    }

    #[test]
    fn should_remove_nested_boundaries_of_the_fallback_whole() {
        let html = concat!(
            r#"<body><div><!--$?--><template id="B:2"></template><!--$--><i>a</i><!--/$-->"#,
            r#"<i>b</i><!--/$--><i>after</i></div>"#,
            r#"<div hidden id="S:5"><b>content</b></div><script>$RR("B:2","S:5",[])</script></body>"#,
        );

        assert_eq!(
            "<div><!--$?--><b>content</b><!--/$--><i>after</i></div>",
            placed(html)
        );
    }

    #[test]
    fn should_leave_a_segment_hidden_when_no_call_or_no_boundary_names_it() {
        let html = concat!(
            r#"<body><div hidden id="S:0"><p>unplaced</p></div>"#,
            r#"<div hidden id="S:1"><p>orphan</p></div><script>$RC("B:9","S:1")</script>"#,
            r#"<script src="/app.js"></script></body>"#,
        );

        assert_eq!(
            concat!(
                r#"<div hidden="" id="S:0"><p>unplaced</p></div>"#,
                r#"<div hidden="" id="S:1"><p>orphan</p></div>"#
            ),
            placed(html)
        );
    }

    #[test]
    fn should_read_only_the_calls_of_inline_scripts() {
        let document = parse_html().one(concat!(
            r#"<body><p>$RC("B:0","S:0")</p>"#,
            r#"<script>self.__next_f.push([1,"$RC(\"B:1\",\"S:1\")"]);$RS("S:2","P:2")</script>"#,
            r#"</body>"#,
        ));

        assert_eq!(
            vec![SegmentMove::CompleteSegment {
                segment: "S:2".to_owned(),
                placeholder: "P:2".to_owned()
            }],
            segment_moves(&document)
        );
    }
}

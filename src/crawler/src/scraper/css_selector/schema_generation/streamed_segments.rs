//! React's streamed server rendering, which Next.js shops use, sends a page's slow parts after its
//! shell: each part arrives as a hidden segment, `<div hidden id="S:…">`, at the end of the body,
//! with an inline script call that moves it into the place it fills. `$RC("B:…","S:…")` (or
//! `$RR`, the same with stylesheets) replaces a Suspense boundary's fallback with the segment, and
//! `$RS("S:…","P:…")` replaces a placeholder `<template>`. A browser runs those calls before anyone
//! reads the page; the projection runs no script, and the markdown view leaves hidden elements
//! out, so without this step a streamed product page's content would be missing from it. The
//! markdown view therefore makes the same moves before it removes the scripts and the comments
//! that delimit the boundaries. The outline and the schema generation prompt do not: selectors run
//! on the page as served, where the segments stay at its end. A segment no call names stays where
//! it is, hidden, as a browser leaves it; one whose boundary or placeholder is gone is dropped, as
//! a browser drops it.

use kuchiki::NodeRef;
use std::collections::HashMap;

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
    let moves = segment_moves(document);
    if moves.is_empty() {
        return;
    }
    let elements = ElementsById::of(document);
    for segment_move in moves {
        match segment_move {
            SegmentMove::CompleteBoundary { boundary, segment } => {
                complete_boundary(&elements, &boundary, &segment)
            }
            SegmentMove::CompleteSegment {
                segment,
                placeholder,
            } => complete_segment(&elements, &segment, &placeholder),
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

/// The page's elements by id, the first in document order for an id used twice, as
/// `getElementById` finds them. Indexed once, before any move: a move keeps every element's id
/// and only takes elements out of the page, which [`ElementsById::attached`] then leaves out.
struct ElementsById(HashMap<String, NodeRef>);

impl ElementsById {
    fn of(document: &NodeRef) -> Self {
        let mut elements = HashMap::new();
        for node in document.descendants() {
            let id = node
                .as_element()
                .and_then(|element| element.attributes.borrow().get("id").map(str::to_owned));
            if let Some(id) = id {
                elements.entry(id).or_insert(node);
            }
        }
        Self(elements)
    }

    /// The element with `id` while it is still in the page: a move may have taken it out with
    /// a removed fallback, as a browser's `getElementById` then no longer finds it.
    fn attached(&self, id: &str) -> Option<&NodeRef> {
        self.0.get(id).filter(|node| {
            node.ancestors()
                .any(|ancestor| ancestor.as_document().is_some())
        })
    }
}

/// `$RC`: takes the segment out of the page, then removes the boundary's content, from its
/// `<template id="B:…">` up to the comment that closes the boundary, and puts the segment's
/// children there. A segment whose boundary is gone is dropped, as a browser drops it.
fn complete_boundary(elements: &ElementsById, boundary_id: &str, segment_id: &str) {
    let Some(segment) = elements.attached(segment_id) else {
        return;
    };
    segment.detach();
    let Some(boundary) = elements.attached(boundary_id) else {
        return;
    };
    let Some(parent) = boundary.parent() else {
        return;
    };
    let boundary_end = remove_boundary_content(boundary);
    for child in segment.children().collect::<Vec<_>>() {
        match &boundary_end {
            Some(end) => end.insert_before(child),
            None => parent.append(child),
        }
    }
}

/// Removes `start` and its following siblings up to the comment that closes the boundary it opens,
/// and returns that comment, or `None` when the boundary is not closed in its parent. Nested
/// boundaries, opened by a comment starting with `$` (`<!--$-->`, `<!--$?-->`, `<!--$!-->` and
/// any later kind), are removed whole.
fn remove_boundary_content(start: &NodeRef) -> Option<NodeRef> {
    let mut depth = 0usize;
    let mut next = Some(start.clone());
    while let Some(node) = next {
        let marker = node.as_comment().map(|comment| comment.borrow().clone());
        match marker.as_deref() {
            Some("/$") if depth == 0 => return Some(node),
            Some("/$") => depth -= 1,
            Some(marker) if marker.starts_with('$') => depth += 1,
            _ => {}
        }
        next = node.next_sibling();
        node.detach();
    }
    None
}

/// `$RS`: takes the segment out of the page and puts its children in the placeholder template's
/// place. A segment whose placeholder is gone is dropped, as a browser drops it.
fn complete_segment(elements: &ElementsById, segment_id: &str, placeholder_id: &str) {
    let Some(segment) = elements.attached(segment_id) else {
        return;
    };
    segment.detach();
    let Some(placeholder) = elements.attached(placeholder_id) else {
        return;
    };
    for child in segment.children().collect::<Vec<_>>() {
        placeholder.insert_before(child);
    }
    placeholder.detach();
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
    fn should_leave_a_segment_no_call_names_and_drop_one_whose_target_is_gone() {
        let html = concat!(
            r#"<body><div hidden id="S:0"><p>unplaced</p></div>"#,
            r#"<div hidden id="S:1"><p>orphan</p></div><script>$RC("B:9","S:1")</script>"#,
            r#"<div hidden id="S:2"><p>orphan</p></div><script>$RS("S:2","P:9")</script>"#,
            r#"<script src="/app.js"></script></body>"#,
        );

        assert_eq!(
            r#"<div hidden="" id="S:0"><p>unplaced</p></div>"#,
            placed(html)
        );
    }

    /// A call naming the segment as its own boundary drops the segment, as React does, and must
    /// not empty the rest of the page.
    #[test]
    fn should_remove_nothing_but_the_segment_when_a_call_names_it_as_its_own_boundary() {
        let html = concat!(
            r#"<body><div hidden id="S:0"><p>segment</p></div><p>kept</p><p>also kept</p>"#,
            r#"<script>$RC("S:0","S:0")</script></body>"#,
        );

        assert_eq!("<p>kept</p><p>also kept</p>", placed(html));
    }

    /// React fills nested boundaries in the order of their calls: an outer segment brings in the
    /// placeholder a later call fills. A boundary without its closing comment takes the segment
    /// at the end of its parent.
    #[test]
    fn should_fill_a_placeholder_an_earlier_segment_brought_in_and_an_unclosed_boundary() {
        let html = concat!(
            r#"<body><main><!--$?--><template id="B:0"></template><p>Loading…</p><!--/$-->"#,
            r#"</main><section><!--$?--><template id="B:1"></template><p>Loading…</p></section>"#,
            r#"<div hidden id="S:0"><h1>Chair</h1><template id="P:1"></template></div>"#,
            r#"<div hidden id="S:1"><p>£50</p></div><div hidden id="S:2"><p>Sold</p></div>"#,
            r#"<script>$RC("B:0","S:0");$RS("S:1","P:1");$RC("B:1","S:2")</script></body>"#,
        );

        assert_eq!(
            concat!(
                "<main><!--$?--><h1>Chair</h1><p>£50</p><!--/$--></main>",
                "<section><!--$?--><p>Sold</p></section>"
            ),
            placed(html)
        );
    }

    /// A placeholder inside a fallback that an earlier move removed is gone, as it is for a
    /// browser's `getElementById`: its segment is dropped, not moved into the removed fallback.
    #[test]
    fn should_drop_a_segment_whose_placeholder_left_the_page_with_a_removed_fallback() {
        let html = concat!(
            r#"<body><main><!--$?--><template id="B:0"></template><div><template id="P:1">"#,
            r#"</template></div><!--/$--></main><div hidden id="S:0"><p>Chair</p></div>"#,
            r#"<div hidden id="S:1"><p>Loading detail</p></div>"#,
            r#"<script>$RC("B:0","S:0");$RS("S:1","P:1")</script></body>"#,
        );

        assert_eq!("<main><!--$?--><p>Chair</p><!--/$--></main>", placed(html));
    }

    #[test]
    fn should_take_at_most_the_first_moves_of_a_page() {
        let calls: String = (0..MAX_SEGMENT_MOVES + 1)
            .map(|index| format!(r#"$RC("B:{index}","S:{index}");"#))
            .collect();
        let document = parse_html().one(format!("<body><script>{calls}</script></body>"));

        assert_eq!(MAX_SEGMENT_MOVES, segment_moves(&document).len());
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

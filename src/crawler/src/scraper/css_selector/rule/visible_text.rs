//! Visible text of a matched element, as a text extraction rule captures it.

/// Elements whose subtree is never rendered as text. Inline `<style>` and `<script>`
/// blocks are descendants of ordinary containers, so a plain descendant text join
/// would leak CSS rules and script source into extracted values.
const NON_TEXT_ELEMENTS: [&str; 4] = ["style", "script", "template", "noscript"];

/// Elements whose text a shop has superseded. Sale prices keep the old amount in a
/// struck-through element next to the current one as a bare text node, which no CSS
/// selector can target, so the old amount is dropped from the containing element's text.
///
/// The skip applies to every text rule, not only to prices: a rule does not know which
/// product field it extracts, so struck-through words inside description or title prose
/// are dropped too. When nothing but struck-through text remains, such as a sold item
/// whose whole title is struck through, the full text is kept instead.
const STRUCK_THROUGH_ELEMENTS: [&str; 3] = ["del", "s", "strike"];

/// Whether text extraction separates the text on either side of a `<br>` or of a block
/// element or table cell boundary.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum TextBoundaries {
    /// Separates it, as a browser lays it out, wherever markup whitespace does not
    /// already: shops write `Top Hat<br>Pen` or minify their markup, and without a
    /// separator the two lines run into one word.
    Separate,
    /// Joins the text nodes as written.
    Ignore,
}

/// How a browser lays out an element's text relative to the text around it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
enum ElementFlow {
    /// Continues the line, as `span`, `a`, `b` or `sup` do, so `£<span>2,800</span>`
    /// and `76 <sup>1</sup>&frasl;<sub>8</sub>` stay one value.
    #[default]
    Inline,
    /// Sits next to the other cells of its table row; a space keeps a row's label and
    /// value on one line.
    Cell,
    /// Starts a line of its own, as `<br>` and block elements do.
    Block,
}

impl ElementFlow {
    fn of(element_name: &str) -> Self {
        match element_name {
            "td" | "th" => Self::Cell,
            "address" | "article" | "aside" | "blockquote" | "br" | "caption" | "dd"
            | "details" | "div" | "dl" | "dt" | "fieldset" | "figcaption" | "figure" | "footer"
            | "form" | "h1" | "h2" | "h3" | "h4" | "h5" | "h6" | "header" | "hr" | "li"
            | "main" | "nav" | "ol" | "p" | "pre" | "section" | "summary" | "table" | "tbody"
            | "tfoot" | "thead" | "tr" | "ul" => Self::Block,
            _ => Self::Inline,
        }
    }

    const fn separator(self) -> Option<char> {
        match self {
            Self::Inline => None,
            Self::Cell => Some(' '),
            Self::Block => Some('\n'),
        }
    }
}

/// The text below `element`, skipping [`NON_TEXT_ELEMENTS`] subtrees and descendant
/// [`STRUCK_THROUGH_ELEMENTS`] subtrees, with its element boundaries handled as
/// `boundaries` says.
pub(super) fn visible_text(
    element: &scraper::ElementRef<'_>,
    boundaries: TextBoundaries,
) -> String {
    // Only descendants are struck-through candidates: a rule that selects `del` itself
    // still extracts its text, while a selected non-text element never does.
    if NON_TEXT_ELEMENTS.contains(&element.value().name()) {
        return String::new();
    }
    let mut text = VisibleText::new(boundaries);
    push_visible_text(element, false, &mut text);
    text.into_value()
}

fn push_visible_text(
    element: &scraper::ElementRef<'_>,
    inside_struck_through: bool,
    text: &mut VisibleText,
) {
    for child in element.children() {
        match child.value() {
            scraper::Node::Text(value) => text.push(value, inside_struck_through),
            scraper::Node::Element(child_element) => {
                let name = child_element.name();
                if NON_TEXT_ELEMENTS.contains(&name) {
                    continue;
                }
                if let Some(child_element) = scraper::ElementRef::wrap(child) {
                    let inside_struck_through =
                        inside_struck_through || STRUCK_THROUGH_ELEMENTS.contains(&name);
                    let flow = ElementFlow::of(name);
                    text.mark_boundary(flow);
                    push_visible_text(&child_element, inside_struck_through, text);
                    text.mark_boundary(flow);
                }
            }
            _ => {}
        }
    }
}

/// Visible text of an element, kept with and without its struck-through descendants.
struct VisibleText {
    boundaries: TextBoundaries,
    without_struck_through: SeparatedText,
    with_struck_through: SeparatedText,
    after_dropped_text: bool,
}

impl VisibleText {
    fn new(boundaries: TextBoundaries) -> Self {
        Self {
            boundaries,
            without_struck_through: SeparatedText::default(),
            with_struck_through: SeparatedText::default(),
            after_dropped_text: false,
        }
    }

    fn mark_boundary(&mut self, flow: ElementFlow) {
        if self.boundaries == TextBoundaries::Separate {
            self.without_struck_through.mark_boundary(flow);
            self.with_struck_through.mark_boundary(flow);
        }
    }

    fn push(&mut self, value: &str, struck_through: bool) {
        self.with_struck_through.push(value);
        if struck_through {
            self.after_dropped_text = true;
            return;
        }
        // `$25 <del>$50</del> ARS` must not leave the double space of the dropped amount.
        let value = if self.after_dropped_text && self.without_struck_through.ends_with_whitespace()
        {
            value.trim_start()
        } else {
            value
        };
        if !value.is_empty() {
            self.after_dropped_text = false;
        }
        self.without_struck_through.push(value);
    }

    fn into_value(self) -> String {
        if self.without_struck_through.text.trim().is_empty() {
            self.with_struck_through.text
        } else {
            self.without_struck_through.text
        }
    }
}

/// Text nodes joined in document order, with one separator between two nodes where an
/// element boundary lies between them and neither node's whitespace separates them.
#[derive(Default)]
struct SeparatedText {
    text: String,
    /// The widest flow of the element boundaries passed since the last text node.
    pending_boundary: ElementFlow,
}

impl SeparatedText {
    fn mark_boundary(&mut self, flow: ElementFlow) {
        self.pending_boundary = self.pending_boundary.max(flow);
    }

    fn push(&mut self, value: &str) {
        if value.is_empty() {
            return;
        }
        let boundary = std::mem::take(&mut self.pending_boundary);
        if let Some(separator) = boundary.separator()
            && !self.text.is_empty()
            && !self.ends_with_whitespace()
            && !value.starts_with(char::is_whitespace)
        {
            self.text.push(separator);
        }
        self.text.push_str(value);
    }

    fn ends_with_whitespace(&self) -> bool {
        self.text.ends_with(char::is_whitespace)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::rstest;
    use scraper::Html;

    /// The visible text of a fragment, trimmed as text extraction trims it.
    fn text_of(markup: &str, boundaries: TextBoundaries) -> String {
        let html = Html::parse_fragment(markup);
        visible_text(&html.root_element(), boundaries)
            .trim()
            .to_owned()
    }

    fn separated_text_of(markup: &str) -> String {
        text_of(markup, TextBoundaries::Separate)
    }

    mod line_breaks_without_surrounding_whitespace {
        use super::*;

        /// Markup of robertyoungantiques.com and brownrigg-interiors.co.uk product pages,
        /// whose lines ran into one word.
        #[rstest]
        #[case::miniature(
            "<p>Depicting a Man in a Black Coat Holding a Top Hat<br>Pen, Ink and Watercolour on Paper <br>13 ¾\" high x 11 ¾\" wide (framed)</p>",
            "Depicting a Man in a Black Coat Holding a Top Hat\nPen, Ink and Watercolour on Paper 13 ¾\" high x 11 ¾\" wide (framed)"
        )]
        #[case::chairs(
            "<p>With Broad Crest Rails, Twin Bar Backs,<br>Saddle Seats and Crinoline Stretchers<br>Well Burnished and Finely Figured Ash and Yew Wood<br>English, c.1830</p>",
            "With Broad Crest Rails, Twin Bar Backs,\nSaddle Seats and Crinoline Stretchers\nWell Burnished and Finely Figured Ash and Yew Wood\nEnglish, c.1830"
        )]
        #[case::measurements(
            "<p>Height: 53cm (20.9 inches)<br/>Width: 65cm</p>",
            "Height: 53cm (20.9 inches)\nWidth: 65cm"
        )]
        fn should_start_a_new_line_at_br_when_no_whitespace_surrounds_it(
            #[case] markup: &str,
            #[case] expected: &str,
        ) {
            assert_eq!(separated_text_of(markup), expected);
        }

        #[test]
        fn should_keep_markup_whitespace_when_it_already_separates_the_lines() {
            assert_eq!(
                separated_text_of("<p>Oak chair<br>\n  Circa 1880 <br> Restored</p>"),
                "Oak chair\n  Circa 1880  Restored"
            );
        }

        #[test]
        fn should_add_one_separator_when_boundaries_follow_each_other() {
            assert_eq!(
                separated_text_of("<div><p>Oak<br><br></p></div><div><p>Circa 1880</p></div>"),
                "Oak\nCirca 1880"
            );
        }
    }

    mod nested_blocks {
        use super::*;

        #[test]
        fn should_separate_the_text_of_nested_block_elements() {
            let markup = "<div><div><p>Oak bureau</p><ul><li>Height 40 cm</li><li>Width 30 cm</li></ul></div><h3>Provenance</h3><p>Private collection</p></div>";

            assert_eq!(
                separated_text_of(markup),
                "Oak bureau\nHeight 40 cm\nWidth 30 cm\nProvenance\nPrivate collection"
            );
        }

        #[test]
        fn should_keep_a_table_row_on_one_line_when_cells_are_minified() {
            let markup = "<table><tr><th>Height</th><td>53 cm</td></tr><tr><th>Width</th><td>65 cm</td></tr></table>";

            assert_eq!(separated_text_of(markup), "Height 53 cm\nWidth 65 cm");
        }
    }

    mod inline_elements {
        use super::*;

        #[rstest]
        #[case::fraction(
            "<span>76 <sup>1</sup>&frasl;<sub>8</sub> in</span>",
            "76 1\u{2044}8 in"
        )]
        #[case::price("<span>£<span>2,800</span></span>", "£2,800")]
        #[case::emphasis("<p><a href=\"/oak\">Oak</a><b>wood</b> chair</p>", "Oakwood chair")]
        fn should_not_separate_inline_elements(#[case] markup: &str, #[case] expected: &str) {
            assert_eq!(separated_text_of(markup), expected);
        }
    }

    mod struck_through_text_at_a_line_break {
        use super::*;

        #[rstest]
        #[case::no_whitespace("<div>£25<del>£50</del><br>VAT included</div>", "£25\nVAT included")]
        #[case::gap_before_dropped_text(
            "<div>£25 <del>£50</del><br>VAT included</div>",
            "£25 VAT included"
        )]
        #[case::gap_on_both_sides(
            "<div>£25 <del>£50</del><br> VAT included</div>",
            "£25 VAT included"
        )]
        #[case::dropped_first_line("<div><del>£50</del><br>£25</div>", "£25")]
        fn should_drop_struck_through_text_and_close_its_gap(
            #[case] markup: &str,
            #[case] expected: &str,
        ) {
            assert_eq!(separated_text_of(markup), expected);
        }

        #[test]
        fn should_separate_struck_through_lines_when_they_are_the_whole_text() {
            assert_eq!(
                separated_text_of("<h1><s>Victorian chair<br>Sold</s></h1>"),
                "Victorian chair\nSold"
            );
        }
    }

    mod ignored_boundaries {
        use super::*;

        #[test]
        fn should_join_text_nodes_as_written_when_boundaries_are_ignored() {
            assert_eq!(
                text_of(
                    "<span>SKU:<br>1234</span><p>-B</p><table><tr><td>7</td></tr></table>",
                    TextBoundaries::Ignore
                ),
                "SKU:1234-B7"
            );
        }
    }
}

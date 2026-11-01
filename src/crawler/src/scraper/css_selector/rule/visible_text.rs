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

/// Classes that the common themes and CSS frameworks (WordPress and WooCommerce,
/// Bootstrap, Tailwind, HTML5 Boilerplate) use to hide text visually while leaving it to
/// screen readers. WooCommerce writes `Original price was: £3,450.` into such an element
/// next to the visible sale price.
const VISUALLY_HIDDEN_CLASSES: [&str; 4] = [
    "screen-reader-text",
    "sr-only",
    "visually-hidden",
    "visuallyhidden",
];

/// How text extraction lays out the text nodes below an element.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum TextLayout {
    /// As a browser renders it: visually hidden descendants are dropped (see
    /// [`is_visually_hidden`] and [`accessible_copy`]), a `<br>` or block boundary starts
    /// a new line, a table cell boundary adds a space, and any other run of HTML
    /// whitespace, such as markup indentation, collapses to one space. Shops write
    /// `Top Hat<br>Pen` or minify their markup, and without a separator the two lines run
    /// into one word. Non-breaking spaces are text, as a browser keeps them.
    Rendered,
    /// Joins the text nodes as written, hidden text and whitespace included.
    AsWritten,
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
            "address" | "article" | "aside" | "blockquote" | "br" | "caption" | "center" | "dd"
            | "details" | "dir" | "div" | "dl" | "dt" | "fieldset" | "figcaption" | "figure"
            | "footer" | "form" | "h1" | "h2" | "h3" | "h4" | "h5" | "h6" | "header" | "hgroup"
            | "hr" | "legend" | "li" | "main" | "menu" | "nav" | "ol" | "p" | "pre" | "search"
            | "section" | "summary" | "table" | "tbody" | "tfoot" | "thead" | "tr" | "ul" => {
                Self::Block
            }
            _ => Self::Inline,
        }
    }
}

/// The text below `element`, skipping [`NON_TEXT_ELEMENTS`] subtrees and descendant
/// [`STRUCK_THROUGH_ELEMENTS`] subtrees, laid out as `layout` says.
pub(super) fn visible_text(element: &scraper::ElementRef<'_>, layout: TextLayout) -> String {
    // Only descendants are struck-through or hidden candidates: a rule that selects `del`
    // or a hidden element itself still extracts its text, while a selected non-text
    // element never does.
    if NON_TEXT_ELEMENTS.contains(&element.value().name()) {
        return String::new();
    }
    let mut text = VisibleText::new(layout);
    push_visible_text(element, TextContext::default(), &mut text);
    text.into_value()
}

/// Whether a text rule extracts a value from `element`. One without visible text yields
/// a blank value, which `first` cardinality skips.
pub(super) fn has_visible_text(element: &scraper::ElementRef<'_>) -> bool {
    !visible_text(element, TextLayout::Rendered)
        .trim()
        .is_empty()
}

/// The elements a text node sits in that change how it is captured.
#[derive(Clone, Copy, Debug, Default)]
struct TextContext {
    /// Inside one of the [`STRUCK_THROUGH_ELEMENTS`].
    struck_through: bool,
    /// Inside a `<pre>`, whose line breaks a browser keeps.
    preformatted: bool,
}

impl TextContext {
    fn inside(self, element_name: &str) -> Self {
        Self {
            struck_through: self.struck_through || STRUCK_THROUGH_ELEMENTS.contains(&element_name),
            preformatted: self.preformatted || element_name == "pre",
        }
    }
}

fn push_visible_text(
    element: &scraper::ElementRef<'_>,
    context: TextContext,
    text: &mut VisibleText,
) {
    for child in element.children() {
        match child.value() {
            scraper::Node::Text(value) => text.push(value, context),
            scraper::Node::Element(child_element) => {
                let name = child_element.name();
                let rendered = text.layout == TextLayout::Rendered;
                if NON_TEXT_ELEMENTS.contains(&name)
                    || (rendered && is_visually_hidden(child_element))
                {
                    continue;
                }
                if let Some(child_element) = scraper::ElementRef::wrap(child) {
                    let source = rendered
                        .then(|| accessible_copy(&child_element))
                        .flatten()
                        .unwrap_or(child_element);
                    let flow = ElementFlow::of(name);
                    text.mark_boundary(flow);
                    push_visible_text(&source, context.inside(name), text);
                    text.mark_boundary(flow);
                }
            }
            _ => {}
        }
    }
}

/// Whether a browser hides `element` from view: it carries the `hidden` attribute, one of
/// the [`VISUALLY_HIDDEN_CLASSES`], or an inline `display: none`.
///
/// `hidden="until-found"` does not count: it marks collapsed content, such as an accordion
/// section, that the browser shows when a visitor searches the page or follows a link.
/// Collapsed tab panels and accordions hidden any other way are dropped, unless the rule
/// selects the panel itself.
///
/// `aria-hidden="true"` does not count: it hides an element from screen readers, not from
/// view, and WooCommerce marks the visible sale price `<ins aria-hidden="true">` next to
/// its screen-reader copy. Stylesheet rules and framework utility classes such as
/// `hidden` or `d-none` do not count either, because responsive variants show the
/// element again on other screen sizes.
fn is_visually_hidden(element: &scraper::node::Element) -> bool {
    element
        .attr("hidden")
        .is_some_and(|value| !value.trim().eq_ignore_ascii_case("until-found"))
        || is_screen_reader_only(element)
        || element.attr("style").is_some_and(declares_display_none)
}

/// Whether `element` is hidden from view but left to screen readers.
fn is_screen_reader_only(element: &scraper::node::Element) -> bool {
    element
        .classes()
        .any(|class| VISUALLY_HIDDEN_CLASSES.contains(&class))
}

/// The screen-reader-only copy that stands in for `element`, an `aria-hidden="true"`
/// element: its next sibling, past whitespace and comments, when that sibling is
/// [screen-reader-only](is_screen_reader_only) and only adds separators to the visible
/// text (see [`adds_only_separators`]).
///
/// Shops render a price for the eye and repeat it for screen readers, as Shopify themes
/// do with `<span aria-hidden="true">€11.327<sup>95</sup></span><span
/// class="visually-hidden">€11.327,95</span>`. The visible text, `€11.32795`, lost the
/// separator that the superscript stood for, so the copy is captured in its place. A copy
/// that adds words, such as WooCommerce's `Current price is: £2,950.`, or that moves or
/// drops a separator is not the same text and stays dropped, as does one that follows a
/// struck-through element.
fn accessible_copy<'a>(element: &scraper::ElementRef<'a>) -> Option<scraper::ElementRef<'a>> {
    let aria_hidden = element
        .value()
        .attr("aria-hidden")
        .is_some_and(|value| value.trim().eq_ignore_ascii_case("true"));
    if !aria_hidden {
        return None;
    }
    let next = element
        .next_siblings()
        .find(|sibling| match sibling.value() {
            scraper::Node::Text(value) => !value.trim().is_empty(),
            scraper::Node::Comment(_) => false,
            _ => true,
        })?;
    let copy = scraper::ElementRef::wrap(next)?;
    if !is_screen_reader_only(copy.value()) {
        return None;
    }
    let visible = visible_text(element, TextLayout::Rendered);
    let copy_text = visible_text(&copy, TextLayout::Rendered);
    (visible.chars().any(char::is_alphanumeric) && adds_only_separators(&visible, &copy_text))
        .then_some(copy)
}

/// Whether `copy` is `visible` with characters other than letters and digits inserted, so
/// both keep every character of `visible` in its place: `€11.327,95` adds a separator to
/// `€11.32795`, while `£29.50` moves the one in `£2,950`.
fn adds_only_separators(visible: &str, copy: &str) -> bool {
    let mut visible = visible.chars().peekable();
    for character in copy.chars() {
        if visible.peek() == Some(&character) {
            visible.next();
        } else if character.is_alphanumeric() {
            return false;
        }
    }
    visible.next().is_none()
}

/// Whether an inline `style` attribute sets `display` to `none`.
fn declares_display_none(style: &str) -> bool {
    style
        .split(';')
        .filter_map(|declaration| declaration.split_once(':'))
        .any(|(property, value)| {
            property.trim().eq_ignore_ascii_case("display")
                && value
                    .trim()
                    .trim_end_matches("!important")
                    .trim_end()
                    .eq_ignore_ascii_case("none")
        })
}

/// Visible text of an element, kept with and without its struck-through descendants.
struct VisibleText {
    layout: TextLayout,
    without_struck_through: LaidOutText,
    with_struck_through: LaidOutText,
    after_dropped_text: bool,
}

impl VisibleText {
    fn new(layout: TextLayout) -> Self {
        Self {
            layout,
            without_struck_through: LaidOutText::new(layout),
            with_struck_through: LaidOutText::new(layout),
            after_dropped_text: false,
        }
    }

    fn mark_boundary(&mut self, flow: ElementFlow) {
        self.without_struck_through.mark_boundary(flow);
        self.with_struck_through.mark_boundary(flow);
    }

    fn push(&mut self, value: &str, context: TextContext) {
        self.with_struck_through.push(value, context.preformatted);
        if context.struck_through {
            self.after_dropped_text = true;
            return;
        }
        // `$25 <del>$50</del> ARS` must not leave the double space of the dropped amount.
        // Rendered text collapses that space itself.
        let value = if self.layout == TextLayout::AsWritten
            && self.after_dropped_text
            && self.without_struck_through.ends_with_whitespace()
        {
            value.trim_start()
        } else {
            value
        };
        if !value.is_empty() {
            self.after_dropped_text = false;
        }
        self.without_struck_through
            .push(value, context.preformatted);
    }

    fn into_value(self) -> String {
        let without_struck_through = self.without_struck_through.into_text();
        if without_struck_through.trim().is_empty() {
            self.with_struck_through.into_text()
        } else {
            without_struck_through
        }
    }
}

/// Text nodes joined in document order and laid out as [`TextLayout`] says.
struct LaidOutText {
    layout: TextLayout,
    text: String,
    /// [`TextLayout::Rendered`] only: the widest gap, whitespace or element boundary,
    /// passed since the last visible character. It is written before the next one, so the
    /// text is never padded with whitespace at either end.
    pending_gap: ElementFlow,
    /// [`TextLayout::Rendered`] only: whether a whitespace character, rather than only
    /// inline element boundaries, stands in the pending gap.
    pending_whitespace: bool,
}

impl LaidOutText {
    fn new(layout: TextLayout) -> Self {
        Self {
            layout,
            text: String::new(),
            pending_gap: ElementFlow::Inline,
            pending_whitespace: false,
        }
    }

    fn mark_boundary(&mut self, flow: ElementFlow) {
        if self.layout == TextLayout::Rendered {
            self.pending_gap = self.pending_gap.max(flow);
        }
    }

    /// Appends a text node. In [`TextLayout::Rendered`], a line break inside a `<pre>`
    /// (`preformatted`) starts a new line as a `<br>` would.
    fn push(&mut self, value: &str, preformatted: bool) {
        match self.layout {
            TextLayout::AsWritten => self.text.push_str(value),
            TextLayout::Rendered => {
                for character in value.chars() {
                    if preformatted && character == '\n' {
                        self.mark_boundary(ElementFlow::Block);
                    } else if character.is_ascii_whitespace() {
                        self.pending_whitespace = true;
                    } else {
                        self.write_pending_gap();
                        self.text.push(character);
                    }
                }
            }
        }
    }

    fn write_pending_gap(&mut self) {
        let gap = std::mem::take(&mut self.pending_gap);
        let whitespace = std::mem::take(&mut self.pending_whitespace);
        if self.text.is_empty() {
            return;
        }
        match gap {
            ElementFlow::Block => self.start_line(),
            ElementFlow::Cell => self.text.push(' '),
            ElementFlow::Inline if whitespace => self.text.push(' '),
            ElementFlow::Inline => {}
        }
    }

    /// Ends the current line. A line of only non-breaking spaces, such as the spacer
    /// paragraph `<p>&nbsp;</p>` that editors write, is removed rather than left as a
    /// blank line.
    fn start_line(&mut self) {
        let line_start = self.text.rfind('\n').map_or(0, |index| index + 1);
        if self.text[line_start..].trim().is_empty() {
            self.text.truncate(line_start);
        }
        if !self.text.is_empty() && !self.text.ends_with('\n') {
            self.text.push('\n');
        }
    }

    fn ends_with_whitespace(&self) -> bool {
        self.text.ends_with(char::is_whitespace)
    }

    fn into_text(self) -> String {
        self.text
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::rstest;
    use scraper::Html;

    /// The visible text of a fragment, trimmed as text extraction trims it.
    fn text_of(markup: &str, layout: TextLayout) -> String {
        let html = Html::parse_fragment(markup);
        visible_text(&html.root_element(), layout).trim().to_owned()
    }

    fn separated_text_of(markup: &str) -> String {
        text_of(markup, TextLayout::Rendered)
    }

    mod line_breaks {
        use super::*;

        /// Markup of robertyoungantiques.com and brownrigg-interiors.co.uk product pages,
        /// whose lines ran into one word.
        #[rstest]
        #[case::miniature(
            "<p>Depicting a Man in a Black Coat Holding a Top Hat<br>Pen, Ink and Watercolour on Paper <br>13 ¾\" high x 11 ¾\" wide (framed)</p>",
            "Depicting a Man in a Black Coat Holding a Top Hat\nPen, Ink and Watercolour on Paper\n13 ¾\" high x 11 ¾\" wide (framed)"
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
        fn should_start_a_new_line_at_br_when_markup_whitespace_surrounds_it() {
            assert_eq!(
                separated_text_of("<p>Oak chair<br>\n  Circa 1880 <br> Restored</p>"),
                "Oak chair\nCirca 1880\nRestored"
            );
        }

        /// Descriptions keep one line break between paragraphs, never a blank line, so
        /// consecutive boundaries and the indentation between them leave one break.
        #[test]
        fn should_add_one_line_break_when_boundaries_follow_each_other() {
            assert_eq!(
                separated_text_of(
                    "<div><p>Oak<br><br></p></div>\n\n  <div>\n    <p>Circa 1880</p></div>"
                ),
                "Oak\nCirca 1880"
            );
        }
    }

    mod whitespace_runs {
        use super::*;

        /// Description editors separate paragraphs with `<p>&nbsp;</p>`.
        #[rstest]
        #[case::between_paragraphs("<div><p>Oak</p><p>&nbsp;</p><p>Circa 1880</p></div>")]
        #[case::around_a_break("<div><p>Oak</p><p> &nbsp; <br>&nbsp;</p><p>Circa 1880</p></div>")]
        #[case::first_and_last_line(
            "<div><p>&nbsp;</p><p>Oak</p><p>Circa 1880</p><p>&nbsp;</p></div>"
        )]
        fn should_drop_a_line_of_only_non_breaking_spaces(#[case] markup: &str) {
            assert_eq!(separated_text_of(markup), "Oak\nCirca 1880");
        }

        #[test]
        fn should_keep_the_line_breaks_of_preformatted_text() {
            assert_eq!(
                separated_text_of("<div><pre>Height:   53 cm\nWidth: 65 cm\n\n</pre>Oak</div>"),
                "Height: 53 cm\nWidth: 65 cm\nOak"
            );
        }

        /// A breadcrumb of a live capture, indented across text nodes and inline elements.
        #[test]
        fn should_collapse_markup_indentation_to_one_space() {
            let markup = "<span>\n      Antique Silver\n      <a href=\"/spoons\">\n        Early English   Antique Spoons\n      </a>\n    </span>";

            assert_eq!(
                separated_text_of(markup),
                "Antique Silver Early English Antique Spoons"
            );
        }

        #[test]
        fn should_collapse_tabs_but_keep_non_breaking_spaces() {
            assert_eq!(
                separated_text_of("<p>6\u{202f}900\u{a0}€\t\t VAT</p>"),
                "6\u{202f}900\u{a0}€ VAT"
            );
        }

        #[test]
        fn should_keep_no_whitespace_when_the_text_is_only_whitespace() {
            assert_eq!(
                visible_text(
                    &Html::parse_fragment("<p> \n <span>\t</span> </p>").root_element(),
                    TextLayout::Rendered
                ),
                ""
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

        /// Block elements shops use less often than `div` or `p`, including the obsolete
        /// `center` and `dir`, which browsers still lay out as blocks.
        #[rstest]
        #[case::center("center")]
        #[case::dir("dir")]
        #[case::hgroup("hgroup")]
        #[case::legend("legend")]
        #[case::menu("menu")]
        #[case::search("search")]
        fn should_start_a_new_line_at_the_boundaries_of_a_block_element(#[case] element: &str) {
            let markup = format!("<div>Oak chair<{element}>Circa 1880</{element}>Restored</div>");

            assert_eq!(
                separated_text_of(&markup),
                "Oak chair\nCirca 1880\nRestored"
            );
        }

        #[test]
        fn should_keep_a_table_row_on_one_line_when_cells_are_minified() {
            let markup = "<table><tr><th>Height</th><td>53 cm</td></tr><tr><th>Width</th><td>65 cm</td></tr></table>";

            assert_eq!(separated_text_of(markup), "Height 53 cm\nWidth 65 cm");
        }

        /// The widest boundary between two text nodes decides their separator, so a cell
        /// boundary next to a block boundary starts a new line instead of adding a space.
        #[rstest]
        #[case::block_opens_the_next_cell(
            "<table><tr><th>Height</th><td><p>53 cm</p></td></tr></table>",
            "Height\n53 cm"
        )]
        #[case::block_closes_the_previous_cell(
            "<table><tr><th><div>Height</div></th><td>53 cm</td></tr></table>",
            "Height\n53 cm"
        )]
        fn should_start_a_new_line_where_a_cell_boundary_meets_a_block_boundary(
            #[case] markup: &str,
            #[case] expected: &str,
        ) {
            assert_eq!(separated_text_of(markup), expected);
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
            "£25\nVAT included"
        )]
        #[case::gap_on_both_sides(
            "<div>£25 <del>£50</del><br> VAT included</div>",
            "£25\nVAT included"
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

    mod visually_hidden_text {
        use super::*;

        /// WooCommerce sale price markup (georgianantiques.net): both amounts are marked
        /// `aria-hidden` for screen readers, which read the hidden sentences instead.
        const WOOCOMMERCE_SALE_PRICE: &str = "<p class=\"price\"><del aria-hidden=\"true\"><span class=\"woocommerce-Price-amount amount\"><bdi><span class=\"woocommerce-Price-currencySymbol\">&pound;</span>3,450</bdi></span></del> <span class=\"screen-reader-text\">Original price was: &pound;3,450.</span><ins aria-hidden=\"true\"><span class=\"woocommerce-Price-amount amount\"><bdi><span class=\"woocommerce-Price-currencySymbol\">&pound;</span>2,950</bdi></span></ins><span class=\"screen-reader-text\">Current price is: &pound;2,950.</span></p>";

        #[test]
        fn should_extract_the_current_price_when_woocommerce_adds_screen_reader_text() {
            assert_eq!(separated_text_of(WOOCOMMERCE_SALE_PRICE), "£2,950");
        }

        #[rstest]
        #[case::screen_reader_text("<span class=\"screen-reader-text\">Price:</span>")]
        #[case::sr_only("<span class=\"label sr-only\">Price:</span>")]
        #[case::visually_hidden("<span class=\"visually-hidden\">Price:</span>")]
        #[case::visuallyhidden("<span class=\"visuallyhidden\">Price:</span>")]
        #[case::hidden_attribute("<span hidden>Price:</span>")]
        #[case::hidden_block("<div hidden><p>Price:</p></div>")]
        #[case::display_none("<span style=\"color: red; display:none\">Price:</span>")]
        #[case::display_none_important("<span style=\"DISPLAY: None !important;\">Price:</span>")]
        fn should_drop_visually_hidden_descendants(#[case] hidden: &str) {
            let markup = format!("<div>{hidden}<span>£2,950</span></div>");

            assert_eq!(separated_text_of(&markup), "£2,950");
        }

        #[rstest]
        #[case::aria_hidden("<span aria-hidden=\"true\">£2,950</span>")]
        #[case::display_block("<span style=\"display: block\">£2,950</span>")]
        #[case::other_property_none("<span style=\"text-decoration: none\">£2,950</span>")]
        #[case::utility_class("<span class=\"hidden md:inline\">£2,950</span>")]
        #[case::hidden_until_found("<span hidden=\"until-found\">£2,950</span>")]
        #[case::class_name_prefix("<span class=\"sr-only-label\">£2,950</span>")]
        fn should_keep_text_that_is_not_visually_hidden(#[case] visible: &str) {
            let markup = format!("<div>{visible}</div>");

            assert_eq!(separated_text_of(&markup), "£2,950");
        }

        /// nicholaswells.com (Shopify) renders the cents as a superscript for the eye and
        /// repeats the price with its separator for screen readers.
        #[test]
        fn should_take_the_screen_reader_copy_when_it_spells_the_visible_text() {
            let markup = "<div class=\"block-price__regular\"><span class=\"visually-hidden\">\n  Regular price\n</span>\n <span class=\"product__price\">\n<span aria-hidden=\"true\">€11.327<sup>95</sup></span>\n<!-- copy -->\n<span class=\"visually-hidden\">€11.327,95</span></span></div>";

            assert_eq!(separated_text_of(markup), "€11.327,95");
        }

        #[rstest]
        #[case::adds_words(
            "<span aria-hidden=\"true\">£2,950</span><span class=\"sr-only\">Price: £2,950</span>",
            "£2,950"
        )]
        #[case::not_aria_hidden(
            "<span>€11.327<sup>95</sup></span><span class=\"sr-only\">€11.327,95</span>",
            "€11.32795"
        )]
        #[case::moves_a_separator(
            "<span aria-hidden=\"true\">£2,950</span><span class=\"sr-only\">£29.50</span>",
            "£2,950"
        )]
        #[case::not_adjacent(
            "<span aria-hidden=\"true\">€11.327<sup>95</sup></span> / <span class=\"sr-only\">€11.327,95</span>",
            "€11.32795 /"
        )]
        #[case::hidden_from_screen_readers_too(
            "<span aria-hidden=\"true\">€11.327<sup>95</sup></span><span hidden>€11.327,95</span>",
            "€11.32795"
        )]
        #[case::struck_through(
            "<del aria-hidden=\"true\">£3,450</del><span class=\"sr-only\">£3,450</span><ins>£2,950</ins>",
            "£2,950"
        )]
        fn should_keep_the_visible_text_when_no_screen_reader_copy_stands_in_for_it(
            #[case] markup: &str,
            #[case] expected: &str,
        ) {
            assert_eq!(separated_text_of(&format!("<div>{markup}</div>")), expected);
        }

        #[test]
        fn should_extract_a_hidden_element_when_it_is_the_selected_element() {
            let html = Html::parse_fragment("<span class=\"sr-only\">SKU 1234</span>");
            let selected = html
                .select(&scraper::Selector::parse("span").unwrap())
                .next()
                .unwrap();

            assert_eq!(visible_text(&selected, TextLayout::Rendered), "SKU 1234");
        }

        #[test]
        fn should_keep_hidden_text_when_text_is_joined_as_written() {
            assert_eq!(
                text_of(
                    "<span>SKU<span class=\"screen-reader-text\">:</span> 1234</span>",
                    TextLayout::AsWritten
                ),
                "SKU: 1234"
            );
        }
    }

    mod text_joined_as_written {
        use super::*;

        /// A captured identity must not change when the layout rules do.
        #[test]
        fn should_keep_internal_whitespace_when_text_is_joined_as_written() {
            assert_eq!(
                text_of("<span>\n  AB  12\n  -B</span>", TextLayout::AsWritten),
                "AB  12\n  -B"
            );
        }

        #[test]
        fn should_join_text_nodes_as_written_when_boundaries_are_ignored() {
            assert_eq!(
                text_of(
                    "<span>SKU:<br>1234</span><p>-B</p><table><tr><td>7</td></tr></table>",
                    TextLayout::AsWritten
                ),
                "SKU:1234-B7"
            );
        }
    }
}

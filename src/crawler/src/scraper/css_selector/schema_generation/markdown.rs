//! The page as markdown, the review agents' other view of a page beside its outline
//! ([`super::outline`]): the readable content of the same parsed page, without the scripts,
//! styles, navigation, header, footer and comments the outline leaves out, as headings (`#`),
//! paragraphs, list items, table rows (`| a | b |`), the text of links and buttons, and image alt
//! text (`![alt]`). It carries no tags, attributes or URLs, so it is for reading what a page shows,
//! such as a price, a sold label or a description, and never for writing a selector. Elements a
//! browser does not show (`hidden`, `aria-hidden="true"`, an inline `display: none`), templates and
//! form selects are left out as well; texts are whole, not shortened.

use super::outline::within_limits;
use super::projection::{is_removed_dsl_tag, parsed_page};
use kuchiki::NodeRef;

/// The most lines one markdown view holds, the first in document order.
const MARKDOWN_LINE_LIMIT: usize = 2_000;
/// The most characters one markdown view holds; its first lines in document order are kept.
const MARKDOWN_CHAR_BUDGET: usize = 100_000;
/// Elements the markdown leaves out with their content, beside those every view leaves out.
const SKIPPED_TAGS: &[&str] = &["template", "select", "datalist", "iframe", "object"];
/// Elements that flow within a line rather than start one. A custom element (a tag with `-`) is
/// inline too, as a browser shows it; block elements inside an inline one still start lines.
const INLINE_TAGS: &[&str] = &[
    "a", "abbr", "b", "bdi", "bdo", "button", "cite", "code", "data", "del", "dfn", "em", "font",
    "i", "ins", "kbd", "label", "mark", "output", "q", "s", "samp", "small", "span", "strike",
    "strong", "sub", "sup", "time", "u", "var", "wbr",
];

/// The markdown of the page `input`.
pub fn html_to_agent_page_markdown(input: &str) -> String {
    let mut writer = MarkdownWriter::default();
    writer.children(&parsed_page(input));
    writer.flush();
    within_limits(
        writer.lines,
        MARKDOWN_LINE_LIMIT,
        MARKDOWN_CHAR_BUDGET,
        "markdown",
    )
}

/// The lines written so far, the text of the line being written, and the prefixes of the block
/// it is in: `prefix` for its next line, `continuation` for the lines after.
#[derive(Default)]
struct MarkdownWriter {
    lines: Vec<String>,
    pending: String,
    prefix: String,
    continuation: String,
}

impl MarkdownWriter {
    fn children(&mut self, node: &NodeRef) {
        for child in node.children() {
            self.node(&child);
        }
    }

    fn node(&mut self, node: &NodeRef) {
        if let Some(text) = node.as_text() {
            self.pending.push_str(&text.borrow());
            return;
        }
        let Some(element) = node.as_element() else {
            return;
        };
        let tag = element.name.local.to_string();
        if is_removed_dsl_tag(&tag) || SKIPPED_TAGS.contains(&tag.as_str()) || is_hidden(node) {
            return;
        }
        match tag.as_str() {
            "head" => {
                let title = node
                    .select_first("title")
                    .ok()
                    .map(|title| title.as_node().text_contents());
                if let Some(title) = title.as_deref().and_then(normalized) {
                    self.lines.push(format!("Title: {title}"));
                }
            }
            "br" => self.flush(),
            "img" => {
                let alt = element.attributes.borrow().get("alt").and_then(normalized);
                if let Some(alt) = alt {
                    self.pending.push_str(&format!(" ![{alt}] "));
                }
            }
            "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => {
                let level = usize::from(tag.as_bytes()[1] - b'0');
                self.marked_block(&format!("{} ", "#".repeat(level)), "", node);
            }
            "blockquote" => self.marked_block("> ", "> ", node),
            "ul" | "ol" => self.list(node, tag == "ol"),
            "tr" => self.table_row(node),
            // Links and buttons read as separate words, as a shop styles them.
            "a" | "button" => {
                self.pending.push(' ');
                self.children(node);
                self.pending.push(' ');
            }
            _ if INLINE_TAGS.contains(&tag.as_str()) || tag.contains('-') => self.children(node),
            _ => {
                self.flush();
                self.children(node);
                self.flush();
            }
        }
    }

    /// A block whose first line starts with `marker` and whose later lines with `continuation`,
    /// within the block around it.
    fn marked_block(&mut self, marker: &str, continuation: &str, node: &NodeRef) {
        self.flush();
        let prefix = format!("{}{marker}", self.continuation);
        let continuation = format!("{}{continuation}", self.continuation);
        let outer_prefix = std::mem::replace(&mut self.prefix, prefix);
        let outer_continuation = std::mem::replace(&mut self.continuation, continuation);
        self.children(node);
        self.flush();
        self.prefix = outer_prefix;
        self.continuation = outer_continuation;
    }

    /// A list's items as `- ` lines, or numbered for an ordered list, indented under the item of
    /// a list they are nested in.
    fn list(&mut self, node: &NodeRef, ordered: bool) {
        self.flush();
        let mut number = 0;
        for child in node.children() {
            let is_item = child
                .as_element()
                .is_some_and(|element| &*element.name.local == "li");
            if !is_item || is_hidden(&child) {
                self.node(&child);
                continue;
            }
            number += 1;
            let marker = if ordered {
                format!("{number}. ")
            } else {
                "- ".to_owned()
            };
            let continuation = " ".repeat(marker.chars().count());
            self.marked_block(&marker, &continuation, &child);
        }
        self.flush();
    }

    /// A table row as one line of its cells' texts, `| a | b |`; a row of empty cells is left out.
    fn table_row(&mut self, node: &NodeRef) {
        self.flush();
        let cells: Vec<String> = node
            .children()
            .filter(|child| {
                child
                    .as_element()
                    .is_some_and(|element| matches!(&*element.name.local, "td" | "th"))
                    && !is_hidden(child)
            })
            .map(|cell| {
                let mut writer = MarkdownWriter::default();
                writer.children(&cell);
                writer.flush();
                writer.lines.join(" ")
            })
            .collect();
        if cells.iter().any(|cell| !cell.is_empty()) {
            self.pending = format!("| {} |", cells.join(" | "));
            self.flush();
        }
    }

    /// Ends the line being written, when it holds any text, with the block's prefix; a line equal
    /// to the one before it is left out.
    fn flush(&mut self) {
        let text = normalized(&std::mem::take(&mut self.pending));
        let Some(text) = text else {
            return;
        };
        let line = format!("{}{text}", self.prefix);
        if self.lines.last() != Some(&line) {
            self.lines.push(line);
        }
        self.prefix = self.continuation.clone();
    }
}

/// Whether a browser does not show the element `node`.
fn is_hidden(node: &NodeRef) -> bool {
    let Some(element) = node.as_element() else {
        return false;
    };
    let attributes = element.attributes.borrow();
    attributes.contains("hidden")
        || attributes
            .get("aria-hidden")
            .is_some_and(|value| value.trim().eq_ignore_ascii_case("true"))
        || attributes.get("style").is_some_and(|style| {
            style.to_ascii_lowercase().split(';').any(|declaration| {
                declaration.split_whitespace().collect::<String>() == "display:none"
            })
        })
}

fn normalized(raw: &str) -> Option<String> {
    let text = raw.split_whitespace().collect::<Vec<_>>().join(" ");
    (!text.is_empty()).then_some(text)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scraper::css_selector::product_schema_service::html_to_agent_page_outline;

    #[test]
    fn should_write_headings_paragraphs_lists_tables_and_prices_as_markdown() {
        let html = r#"<!doctype html><html><head><title> Oak chair | Shop </title>
            <meta property="og:price:amount" content="120"></head><body>
            <h1 class="product-title">Oak <em>Windsor</em> chair</h1>
            <div class="price-box"><span class="label">Price:</span> <span class="price">£120</span></div>
            <p>A sturdy chair,<br>made in 1890.</p>
            <h2>Details</h2>
            <ul><li>Oak</li><li>Height <b>90 cm</b><ul><li>Seat 45 cm</li></ul></li></ul>
            <ol><li>First</li><li>Second</li></ol>
            <table><tr><th>Maker</th><td>Unknown</td></tr><tr><td></td></tr></table>
            <a href="/cart"><img src="c.jpg" alt="Front view">Add to cart</a>
            <button disabled>Sold out</button>
            </body></html>"#;

        assert_eq!(
            html_to_agent_page_markdown(html),
            "Title: Oak chair | Shop\n\
             # Oak Windsor chair\n\
             Price: £120\n\
             A sturdy chair,\n\
             made in 1890.\n\
             ## Details\n\
             - Oak\n\
             - Height 90 cm\n\
             \x20\x20- Seat 45 cm\n\
             1. First\n\
             2. Second\n\
             | Maker | Unknown |\n\
             ![Front view] Add to cart Sold out\n"
        );
    }

    #[test]
    fn should_leave_out_what_the_outline_leaves_out_and_what_a_browser_hides() {
        let html = r#"<html><head><style>.x{}</style><script>var price = 1;</script></head><body>
            <header><a href="/">Shop home</a></header><nav><ul><li>Chairs</li></ul></nav>
            <header-drawer>Drawer</header-drawer><!-- a comment -->
            <main><h1>Chair</h1><p hidden>Hidden note</p><span style="display: none">Old price</span>
            <span aria-hidden="true">★</span><select><option>EUR</option><option>GBP</option></select>
            <noscript>Enable JavaScript</noscript><template><p>Template</p></template>
            <product-price><span>£50</span></product-price></main>
            <aside>Related</aside><footer>Imprint</footer></body></html>"#;

        assert_eq!(html_to_agent_page_markdown(html), "# Chair\n£50\n");
    }

    #[test]
    fn should_cut_the_markdown_at_its_size_limit_and_say_so() {
        let paragraphs: String = (0..MARKDOWN_LINE_LIMIT + 5)
            .map(|index| format!("<p>Paragraph {index}</p>"))
            .collect();

        let markdown =
            html_to_agent_page_markdown(&format!("<html><body>{paragraphs}</body></html>"));

        assert_eq!(markdown.lines().count(), MARKDOWN_LINE_LIMIT + 1);
        assert!(
            markdown
                .ends_with("… 5 more lines of the page are not shown: the markdown's size limit\n")
        );
    }

    #[test]
    fn should_be_far_shorter_than_the_outline_on_the_parser_fixtures() {
        let fixtures = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/html");
        let mut outline_chars = 0;
        let mut markdown_chars = 0;
        for entry in std::fs::read_dir(fixtures).unwrap_or_else(|error| panic!("{error}")) {
            let path = entry.unwrap_or_else(|error| panic!("{error}")).path();
            let html = std::fs::read_to_string(&path).unwrap_or_else(|error| panic!("{error}"));
            let markdown = html_to_agent_page_markdown(&html);
            let outline = html_to_agent_page_outline(&html);
            assert!(
                !markdown.contains("size limit"),
                "{} was cut",
                path.display()
            );
            outline_chars += outline.chars().count();
            markdown_chars += markdown.chars().count();
        }

        // About a quarter of the outline's size on the fixtures when this was written.
        assert!(
            markdown_chars * 2 < outline_chars,
            "markdown {markdown_chars} characters, outline {outline_chars}"
        );
    }
}

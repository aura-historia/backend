//! The page as Markdown, which the review agents read when they judge what a URL is rather than
//! write selectors for it (a URL-pattern review): the page title, the headings, paragraphs,
//! lists, tables, links and image alternatives of the page the schema generation projection
//! shows, without scripts, styles, navigation, header, footer and form controls. Selectors are
//! no use there, so attributes other than a link's `href` and an image's `alt` and `src` are left
//! out.

use super::projection::cleaned_page;
use kuchiki::NodeRef;

/// The most characters one page holds; its first lines in document order are kept.
const MARKDOWN_CHAR_BUDGET: usize = 100_000;
/// The most characters a link target or an image source keeps.
const TARGET_LIMIT: usize = 200;

/// Tags whose content is no reading text of the page.
const SKIPPED_TAGS: &[&str] = &[
    "head", "template", "iframe", "object", "embed", "input", "select", "textarea", "datalist",
    "video", "audio", "map", "dialog",
];
/// Tags that set their content apart as a block.
const BLOCK_TAGS: &[&str] = &[
    "div", "p", "section", "article", "main", "figure", "figcaption", "form", "fieldset",
    "details", "summary", "address", "dl", "dt", "dd", "center", "body", "html",
];

/// The Markdown of the page `input`.
pub fn html_to_agent_page_markdown(input: &str) -> String {
    let document = cleaned_page(input);
    let mut writer = MarkdownWriter::default();
    if let Some(title) = page_title(&document) {
        writer.text(&format!("Title: {title}"));
        writer.block_break();
    }
    writer.children(&document, 0);
    within_budget(writer.finish(), MARKDOWN_CHAR_BUDGET)
}

fn page_title(document: &NodeRef) -> Option<String> {
    let title = document.select_first("title").ok()?;
    let text = title.text_contents();
    let title = text.split_whitespace().collect::<Vec<_>>().join(" ");
    (!title.is_empty()).then_some(title)
}

/// The lines of `markdown` that fit in `budget` characters, and a last line counting the
/// characters left out when any are.
fn within_budget(markdown: String, budget: usize) -> String {
    if markdown.len() <= budget {
        return markdown;
    }
    let mut end = budget;
    while !markdown.is_char_boundary(end) {
        end -= 1;
    }
    let end = markdown[..end].rfind('\n').map_or(end, |newline| newline + 1);
    let left_out = markdown[end..].chars().count();
    format!(
        "{}… {left_out} more characters of the page are not shown: the size limit\n",
        &markdown[..end]
    )
}

#[derive(Default)]
struct MarkdownWriter {
    out: String,
    /// Whether whitespace came after the last text, to show as one space before the next.
    space_pending: bool,
}

impl MarkdownWriter {
    fn finish(self) -> String {
        let mut out = self.out.trim_end().to_owned();
        out.push('\n');
        if out.trim().is_empty() { String::new() } else { out }
    }

    fn at_line_start(&self) -> bool {
        self.out.is_empty() || self.out.ends_with('\n')
    }

    fn line_break(&mut self) {
        self.space_pending = false;
        while self.out.ends_with(' ') {
            self.out.pop();
        }
        if !self.at_line_start() {
            self.out.push('\n');
        }
    }

    fn block_break(&mut self) {
        self.line_break();
        if !self.out.is_empty() && !self.out.ends_with("\n\n") {
            self.out.push('\n');
        }
    }

    /// Text with its whitespace runs shown as one space.
    fn text(&mut self, raw: &str) {
        let words = raw.split_whitespace().collect::<Vec<_>>().join(" ");
        if words.is_empty() {
            self.space_pending |= !raw.is_empty();
            return;
        }
        let leading = raw.starts_with(char::is_whitespace);
        if (leading || self.space_pending) && !self.at_line_start() && !self.out.ends_with(' ') {
            self.out.push(' ');
        }
        self.out.push_str(&words);
        self.space_pending = raw.ends_with(char::is_whitespace);
    }

    fn children(&mut self, node: &NodeRef, list_depth: usize) {
        for child in node.children() {
            self.node(&child, list_depth);
        }
    }

    fn node(&mut self, node: &NodeRef, list_depth: usize) {
        if let Some(text) = node.as_text() {
            self.text(&text.borrow());
            return;
        }
        let Some(element) = node.as_element() else {
            self.children(node, list_depth);
            return;
        };
        let tag = element.name.local.to_string();
        if SKIPPED_TAGS.contains(&tag.as_str()) || is_hidden(node) {
            return;
        }
        match tag.as_str() {
            "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => {
                let level = usize::from(tag.as_bytes()[1] - b'0');
                let heading = inline_text(node);
                if !heading.is_empty() {
                    self.block_break();
                    self.out.push_str(&format!("{} {heading}", "#".repeat(level)));
                    self.block_break();
                }
            }
            "br" => self.line_break(),
            "hr" => {
                self.block_break();
                self.out.push_str("---");
                self.block_break();
            }
            "ul" | "ol" => self.list(node, tag == "ol", list_depth),
            "li" => self.list_item(node, "- ", list_depth),
            "table" => self.table(node),
            "pre" => {
                let code = node.text_contents();
                if !code.trim().is_empty() {
                    self.block_break();
                    self.out.push_str(&format!("```\n{}\n```", code.trim_matches('\n')));
                    self.block_break();
                }
            }
            "blockquote" => {
                let quote = inline_text(node);
                if !quote.is_empty() {
                    self.block_break();
                    self.out.push_str(&format!("> {quote}"));
                    self.block_break();
                }
            }
            "a" => self.link(node, list_depth),
            "img" => self.image(node),
            "strong" | "b" => self.emphasis(node, "**", list_depth),
            "em" | "i" => self.emphasis(node, "*", list_depth),
            "code" => {
                let code = inline_text(node);
                if !code.is_empty() {
                    self.text(&format!("`{code}`"));
                }
            }
            "button" => {
                let label = inline_text(node);
                if !label.is_empty() {
                    self.text(&format!("[{label}]"));
                }
            }
            tag if BLOCK_TAGS.contains(&tag) => {
                self.block_break();
                self.children(node, list_depth);
                self.block_break();
            }
            _ => self.children(node, list_depth),
        }
    }

    fn list(&mut self, node: &NodeRef, ordered: bool, list_depth: usize) {
        self.line_break();
        if list_depth == 0 {
            self.block_break();
        }
        let mut number = 0;
        for child in node.children() {
            if child.as_element().is_some_and(|el| &*el.name.local == "li") {
                number += 1;
                let marker = if ordered {
                    format!("{number}. ")
                } else {
                    "- ".to_owned()
                };
                self.list_item(&child, &marker, list_depth + 1);
            } else {
                self.node(&child, list_depth + 1);
            }
        }
        self.line_break();
        if list_depth == 0 {
            self.block_break();
        }
    }

    fn list_item(&mut self, node: &NodeRef, marker: &str, list_depth: usize) {
        self.line_break();
        self.out.push_str(&"  ".repeat(list_depth.saturating_sub(1)));
        self.out.push_str(marker);
        self.children(node, list_depth);
        self.line_break();
    }

    fn table(&mut self, node: &NodeRef) {
        self.block_break();
        for row in node.select("tr").into_iter().flatten() {
            let cells: Vec<String> = row
                .as_node()
                .children()
                .filter(|cell| {
                    cell.as_element()
                        .is_some_and(|el| matches!(&*el.name.local, "td" | "th"))
                })
                .map(|cell| inline_text(&cell).replace('|', "\\|"))
                .collect();
            if cells.iter().any(|cell| !cell.is_empty()) {
                self.line_break();
                self.out.push_str(&format!("| {} |", cells.join(" | ")));
            }
        }
        self.block_break();
    }

    fn link(&mut self, node: &NodeRef, list_depth: usize) {
        let label = inline_text(node);
        let target = attribute(node, "href")
            .filter(|href| !href.is_empty() && !href.starts_with('#'))
            .filter(|href| !href.to_ascii_lowercase().starts_with("javascript:"));
        match target {
            Some(target) if !label.is_empty() => {
                self.text(&format!("[{label}]({})", shortened(&target)));
            }
            Some(_) => {
                // A link around an image or an icon: its image shows it.
                self.children(node, list_depth);
            }
            None => self.children(node, list_depth),
        }
    }

    fn image(&mut self, node: &NodeRef) {
        let alt = attribute(node, "alt").unwrap_or_default();
        let source = attribute(node, "src")
            .or_else(|| attribute(node, "data-src"))
            .filter(|source| !source.starts_with("data:"));
        match (alt.is_empty(), source) {
            (false, Some(source)) => {
                self.text(&format!("![{alt}]({})", shortened(&source)));
            }
            (false, None) => self.text(&format!("![{alt}]")),
            (true, Some(source)) => self.text(&format!("![]({})", shortened(&source))),
            (true, None) => {}
        }
    }

    fn emphasis(&mut self, node: &NodeRef, mark: &str, list_depth: usize) {
        let mut inner = MarkdownWriter::default();
        inner.children(node, list_depth);
        let text = inner.finish();
        let text = text.trim();
        if text.is_empty() {
            return;
        }
        if text.contains('\n') {
            self.text(text);
        } else {
            self.text(&format!("{mark}{text}{mark}"));
        }
    }
}

/// The text of `node` on one line, with its links, images and emphasis as Markdown.
fn inline_text(node: &NodeRef) -> String {
    let mut inner = MarkdownWriter::default();
    inner.children(node, 0);
    inner
        .finish()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn attribute(node: &NodeRef, name: &str) -> Option<String> {
    let element = node.as_element()?;
    let attributes = element.attributes.borrow();
    attributes.get(name).map(|value| value.trim().to_owned())
}

/// Whether the page itself hides the element from a reader.
fn is_hidden(node: &NodeRef) -> bool {
    attribute(node, "hidden").is_some()
        || attribute(node, "aria-hidden").is_some_and(|value| value == "true")
        || attribute(node, "type").is_some_and(|value| value == "hidden")
}

fn shortened(target: &str) -> String {
    if target.chars().count() <= TARGET_LIMIT {
        return target.to_owned();
    }
    let mut kept: String = target.chars().take(TARGET_LIMIT).collect();
    kept.push('…');
    kept
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_show_the_title_headings_paragraphs_links_and_images_as_markdown() {
        let html = r#"<html><head><title> Oak chair | Shop </title>
            <script>var a = 1;</script></head><body>
            <nav><a href="/menu">Menu</a></nav>
            <h1 class="title">Oak <em>Windsor</em> chair</h1>
            <p>Made in <strong>1890</strong>. See <a href="/chairs">all chairs</a>.</p>
            <img src="/files/chair.jpg" alt="Front view">
            <footer>Copyright</footer></body></html>"#;

        assert_eq!(
            html_to_agent_page_markdown(html),
            "Title: Oak chair | Shop\n\n\
             # Oak *Windsor* chair\n\n\
             Made in **1890**. See [all chairs](/chairs).\n\n\
             ![Front view](/files/chair.jpg)\n"
        );
    }

    #[test]
    fn should_show_lists_nested_and_tables_as_rows() {
        let html = r#"<body><ul><li>Oak<ul><li>Seat</li><li>Legs</li></ul></li><li>Pine</li></ul>
            <ol><li>First</li><li>Second</li></ol>
            <table><tr><th>Width</th><td>90 cm</td></tr><tr><th>Height</th><td>45 | 50 cm</td></tr></table>
            </body>"#;

        assert_eq!(
            html_to_agent_page_markdown(html),
            "- Oak\n  - Seat\n  - Legs\n- Pine\n\n\
             1. First\n2. Second\n\n\
             | Width | 90 cm |\n| Height | 45 \\| 50 cm |\n"
        );
    }

    #[test]
    fn should_leave_out_hidden_content_form_controls_and_data_uris() {
        let html = r#"<body><div hidden>Secret</div><input type="text" value="x">
            <span aria-hidden="true">Icon</span><select><option>Austria</option></select>
            <img src="data:image/gif;base64,AAAA"><a href="javascript:void(0)">Open</a>
            <a href="#top">Top</a><p>Visible</p></body>"#;

        assert_eq!(html_to_agent_page_markdown(html), "Open Top\n\nVisible\n");
    }

    #[test]
    fn should_keep_the_first_lines_of_a_page_over_the_budget() {
        let page = "# One\n\nTwo\n\nThree\n";

        assert_eq!(
            within_budget(page.to_owned(), 12),
            "# One\n\nTwo\n… 7 more characters of the page are not shown: the size limit\n"
        );
        assert_eq!(within_budget(page.to_owned(), 100), page);
    }

    #[test]
    fn should_answer_nothing_for_a_page_without_content() {
        assert_eq!(html_to_agent_page_markdown("<html><body></body></html>"), "");
    }
}

//! The page outline the review agents read: the elements of the schema generation projection
//! ([`project_page`]), one per line in CSS notation, such as
//! `span.price[itemprop='price'] | £120`, indented one space per level, with the element's text
//! after ` | `. Values are single-quoted, which a JSON answer carries without escaping. It shows the same elements, attributes and texts as the YAML the schema
//! generation prompt reads, in about half the characters, and a selector can be copied from it.
//! A run of siblings with the same structure, such as a currency menu's countries or a gallery's
//! thumbnails, shows its first members and every member whose text holds a digit (a price, a
//! dimension, a date), and one line counts the rest. Agents re-read every page they inspect on
//! each later turn, so its size is what their context costs.

use super::projection::{HtmlYamlNode, is_boolean_projected_attr, project_page};

/// The most lines one outline holds, the first in document order.
const OUTLINE_LINE_LIMIT: usize = 2_000;
/// The most characters one outline holds; its first lines in document order are kept.
const OUTLINE_CHAR_BUDGET: usize = 100_000;
/// The shortest run of same-structure siblings the outline shortens.
const SIBLING_RUN_MIN: usize = 5;
/// The members of a shortened run that are always shown, the first ones.
const SIBLING_RUN_SHOWN: usize = 2;

/// The outline of the page `input`.
pub fn html_to_agent_page_outline(input: &str) -> String {
    let mut lines = Vec::new();
    outline_nodes(&project_page(input), 0, &mut lines);
    within_limits(lines, OUTLINE_LINE_LIMIT, OUTLINE_CHAR_BUDGET, "outline")
}

/// The first `line_limit` lines that fit in `budget` characters, and a last line counting the
/// lines left out when any is, naming the `view` whose limit left them out.
pub(super) fn within_limits(
    lines: Vec<String>,
    line_limit: usize,
    budget: usize,
    view: &str,
) -> String {
    let mut outline = String::new();
    let mut kept = 0;
    for line in &lines {
        if kept == line_limit || outline.len() + line.len() + 1 > budget {
            break;
        }
        outline.push_str(line);
        outline.push('\n');
        kept += 1;
    }
    if kept < lines.len() {
        outline.push_str(&format!(
            "… {} more lines of the page are not shown: the {view}'s size limit\n",
            lines.len() - kept
        ));
    }
    outline
}

fn outline_nodes(nodes: &[HtmlYamlNode], depth: usize, lines: &mut Vec<String>) {
    let mut start = 0;
    while start < nodes.len() {
        let shape = structure(&nodes[start]);
        let run = nodes[start..]
            .iter()
            .take_while(|node| structure(node) == shape)
            .count();
        let members = &nodes[start..start + run];
        if run < SIBLING_RUN_MIN {
            for node in members {
                outline_node(node, depth, lines);
            }
        } else {
            outline_run(members, depth, lines);
        }
        start += run;
    }
}

/// A run of same-structure siblings: the first members and those whose text holds a digit, and
/// a line counting each stretch of members in between.
fn outline_run(members: &[HtmlYamlNode], depth: usize, lines: &mut Vec<String>) {
    let mut hidden = 0;
    for (position, node) in members.iter().enumerate() {
        if position < SIBLING_RUN_SHOWN || has_digit_text(node) {
            hidden_line(hidden, &members[0], depth, lines);
            hidden = 0;
            outline_node(node, depth, lines);
        } else {
            hidden += 1;
        }
    }
    hidden_line(hidden, &members[0], depth, lines);
}

fn hidden_line(hidden: usize, member: &HtmlYamlNode, depth: usize, lines: &mut Vec<String>) {
    if hidden == 0 {
        return;
    }
    lines.push(format!(
        "{}… {hidden} more {}{} with the same structure",
        indent(depth),
        member.tag,
        class_notation(member)
    ));
}

fn outline_node(node: &HtmlYamlNode, depth: usize, lines: &mut Vec<String>) {
    let mut line = indent(depth);
    line.push_str(&node.tag);
    line.push_str(&id_notation(node));
    line.push_str(&class_notation(node));
    for (name, value) in &node.attrs {
        if name == "id" || name == "class" {
            continue;
        }
        if is_boolean_projected_attr(name) {
            line.push_str(&format!("[{name}]"));
        } else {
            line.push_str(&format!("[{name}={}]", quoted(value)));
        }
    }
    if let Some(text) = &node.text {
        line.push_str(" | ");
        line.push_str(text);
    }
    lines.push(line);
    outline_nodes(&node.children, depth + 1, lines);
}

fn indent(depth: usize) -> String {
    " ".repeat(depth)
}

/// `#id`, or `[id="…"]` for an id that is no CSS identifier.
fn id_notation(node: &HtmlYamlNode) -> String {
    match node.attrs.get("id") {
        Some(id) if is_css_identifier(id) => format!("#{id}"),
        Some(id) => format!("[id={}]", quoted(id)),
        None => String::new(),
    }
}

/// `.a.b`, or `[class="…"]` when any class is no CSS identifier, such as `md:flex`.
fn class_notation(node: &HtmlYamlNode) -> String {
    let Some(class) = node.attrs.get("class") else {
        return String::new();
    };
    let classes: Vec<&str> = class.split_whitespace().collect();
    if classes.iter().all(|class| is_css_identifier(class)) {
        classes.iter().map(|class| format!(".{class}")).collect()
    } else {
        format!("[class={}]", quoted(class))
    }
}

/// Whether `name` can follow `.` or `#` in a selector unescaped.
fn is_css_identifier(name: &str) -> bool {
    let rest = name.strip_prefix('-').unwrap_or(name);
    rest.chars()
        .next()
        .is_some_and(|first| first.is_ascii_alphabetic() || first == '_')
        && rest
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || "-_".contains(character))
}

/// `value` as a single-quoted CSS string.
fn quoted(value: &str) -> String {
    let escaped = value.replace('\\', "\\\\").replace('\'', "\\'");
    format!("'{escaped}'")
}

/// The attributes whose values name what an element is, such as a `meta` tag's `property`
/// `og:image`, so siblings that differ in them are never counted away as the same structure.
const IDENTIFYING_ATTRS: &[&str] = &["class", "itemprop", "name", "property", "rel", "type"];

/// The tags, identifying attributes and attribute names of `node` and its descendants: what
/// makes siblings the same structure.
fn structure(node: &HtmlYamlNode) -> String {
    let mut shape = node.tag.clone();
    for (name, value) in &node.attrs {
        shape.push('[');
        shape.push_str(name);
        if IDENTIFYING_ATTRS.contains(&name.as_str()) {
            shape.push('=');
            shape.push_str(value);
        }
        shape.push(']');
    }
    shape.push('(');
    for child in &node.children {
        shape.push_str(&structure(child));
    }
    shape.push(')');
    shape
}

fn has_digit_text(node: &HtmlYamlNode) -> bool {
    node.text
        .as_deref()
        .is_some_and(|text| text.chars().any(|character| character.is_ascii_digit()))
        || node.children.iter().any(has_digit_text)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scraper::css_selector::product_schema_service::html_to_schema_prompt_dsl;

    #[test]
    fn should_show_each_element_in_css_notation_indented_by_its_depth() {
        let html = r#"<html><body><div class="product-main" id="main">
            <h1 class="title" itemprop="name">Oak "Windsor" chair</h1>
            <span class="price md:text-lg">£120</span>
            <div itemscope itemtype="https://schema.org/Offer"><meta itemprop="price" content="120"></div>
            </div></body></html>"#;

        assert_eq!(
            html_to_agent_page_outline(html),
            "div#main.product-main | Oak \"Windsor\" chair £120\n\
             \x20h1.title[itemprop='name'] | Oak \"Windsor\" chair\n\
             \x20span[class='price md:text-lg'] | £120\n\
             \x20div[itemscope][itemtype='https://schema.org/Offer']\n\
             \x20\x20meta[content='120'][itemprop='price']\n"
        );
    }

    #[test]
    fn should_count_same_structure_siblings_after_the_first_but_show_those_with_digits_or_own_names()
     {
        let countries: String = ["Aruba", "Austria", "Belgium", "Chile", "Denmark", "Egypt"]
            .iter()
            .map(|country| format!(r#"<li class="option"><a class="flag">{country}</a></li>"#))
            .collect();
        let rows = r#"<tr><td class="k">Maker</td></tr><tr><td class="k">Oak</td></tr>
            <tr><td class="k">Width</td></tr><tr><td class="k">Height 90 cm</td></tr>
            <tr><td class="k">Wood</td></tr><tr><td class="k">Era</td></tr>"#;
        let metas: String = [
            "og:title",
            "og:image",
            "og:type",
            "og:url",
            "product:price:amount",
        ]
        .iter()
        .map(|property| format!(r#"<meta property="{property}" content="x">"#))
        .collect();
        let html = format!(
            r#"<html><head>{metas}</head><body><ul class="currencies">{countries}</ul><table class="specs">{rows}</table></body></html>"#
        );

        let outline = html_to_agent_page_outline(&html);

        assert_eq!(
            outline
                .lines()
                .filter(|line| line.starts_with("meta"))
                .count(),
            5,
            "{outline}"
        );
        assert_eq!(
            outline.lines().skip(5).collect::<Vec<_>>().join("\n") + "\n",
            "ul.currencies\n\
             \x20li.option\n  a.flag | Aruba\n\
             \x20li.option\n  a.flag | Austria\n\
             \x20… 4 more li.option with the same structure\n\
             table.specs\n\
             \x20tbody\n\
             \x20\x20tr\n   td.k | Maker\n\
             \x20\x20tr\n   td.k | Oak\n\
             \x20\x20… 1 more tr with the same structure\n\
             \x20\x20tr\n   td.k | Height 90 cm\n\
             \x20\x20… 2 more tr with the same structure\n"
        );
    }

    #[test]
    fn should_keep_the_first_lines_within_the_budget_and_count_the_rest() {
        let lines: Vec<String> = (0..10).map(|index| format!("p | {index}")).collect();

        assert_eq!(
            within_limits(lines.clone(), 3, 1_000, "outline"),
            "p | 0\np | 1\np | 2\n… 7 more lines of the page are not shown: the outline's size limit\n"
        );
        assert_eq!(
            within_limits(lines.clone(), 100, 17, "outline"),
            "p | 0\np | 1\n… 8 more lines of the page are not shown: the outline's size limit\n"
        );
        assert_eq!(
            within_limits(lines, 100, 1_000, "outline").lines().count(),
            10
        );
    }

    #[test]
    fn should_outline_every_parser_fixture_in_well_under_its_yaml_projection() {
        let fixtures = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/html");
        let mut yaml_chars = 0;
        let mut outline_chars = 0;
        for entry in std::fs::read_dir(fixtures).unwrap_or_else(|error| panic!("{error}")) {
            let path = entry.unwrap_or_else(|error| panic!("{error}")).path();
            let html = std::fs::read_to_string(&path).unwrap_or_else(|error| panic!("{error}"));
            let outline = html_to_agent_page_outline(&html);
            assert!(
                !outline.contains("not shown: the outline's size limit"),
                "{} was cut",
                path.display()
            );
            yaml_chars += html_to_schema_prompt_dsl(&html).chars().count();
            outline_chars += outline.chars().count();
        }

        assert!(
            outline_chars * 2 < yaml_chars,
            "outlines {outline_chars} characters, YAML {yaml_chars}"
        );
    }

    /// The rules of a fixture schema file: every object with a `selector`, with the attribute it
    /// reads, if any.
    fn fixture_rules(schemas: &serde_json::Value, rules: &mut Vec<(String, Option<String>)>) {
        match schemas {
            serde_json::Value::Object(object) => {
                if let Some(selector) = object.get("selector").and_then(serde_json::Value::as_str) {
                    let attribute = object
                        .get("name")
                        .and_then(serde_json::Value::as_str)
                        .map(str::to_owned);
                    rules.push((selector.to_owned(), attribute));
                }
                object
                    .values()
                    .for_each(|value| fixture_rules(value, rules));
            }
            serde_json::Value::Array(values) => {
                values.iter().for_each(|value| fixture_rules(value, rules));
            }
            _ => {}
        }
    }

    #[test]
    fn should_show_every_value_of_the_fixture_schemas_that_the_yaml_projection_shows() {
        use kuchiki::traits::TendrilSink;
        let fixtures = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
        let mut checked = 0;
        for entry in std::fs::read_dir(fixtures.join("schemas")).unwrap_or_else(|e| panic!("{e}")) {
            let schema_path = entry.unwrap_or_else(|e| panic!("{e}")).path();
            let shop = schema_path
                .file_stem()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned();
            let schemas: serde_json::Value = serde_json::from_str(
                &std::fs::read_to_string(&schema_path).unwrap_or_else(|e| panic!("{e}")),
            )
            .unwrap_or_else(|e| panic!("{e}"));
            let mut rules = Vec::new();
            fixture_rules(&schemas, &mut rules);
            for page in std::fs::read_dir(fixtures.join("html")).unwrap_or_else(|e| panic!("{e}")) {
                let page = page.unwrap_or_else(|e| panic!("{e}")).path();
                if !page
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .starts_with(&format!("{shop}_"))
                {
                    continue;
                }
                let html = std::fs::read_to_string(&page).unwrap_or_else(|e| panic!("{e}"));
                let yaml = html_to_schema_prompt_dsl(&html);
                let outline = html_to_agent_page_outline(&html);
                let document = kuchiki::parse_html().one(html.as_str());
                for (selector, attribute) in &rules {
                    let Ok(mut matches) = document.select(selector) else {
                        continue;
                    };
                    let Some(element) = matches.next() else {
                        continue;
                    };
                    let value = match attribute {
                        Some(name) => element
                            .attributes
                            .borrow()
                            .get(name.as_str())
                            .map(str::to_owned),
                        None => Some(element.text_contents()),
                    };
                    let Some(value) = value else { continue };
                    let probe: String = value
                        .split_whitespace()
                        .collect::<Vec<_>>()
                        .join(" ")
                        .chars()
                        .take(30)
                        .collect();
                    if probe.chars().count() < 3
                        || probe.contains(['\'', '\\'])
                        || !yaml.contains(&probe)
                    {
                        continue;
                    }
                    checked += 1;
                    assert!(
                        outline.contains(&probe),
                        "{}: {selector} reads {probe:?}, which the YAML shows and the outline does not",
                        page.display()
                    );
                }
            }
        }
        assert!(checked >= 20, "only {checked} values checked");
    }
}

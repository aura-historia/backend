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

/// Joins the text nodes below `element`, skipping [`NON_TEXT_ELEMENTS`] subtrees and
/// descendant [`STRUCK_THROUGH_ELEMENTS`] subtrees.
pub(super) fn visible_text(element: &scraper::ElementRef<'_>) -> String {
    // Only descendants are struck-through candidates: a rule that selects `del` itself
    // still extracts its text, while a selected non-text element never does.
    if NON_TEXT_ELEMENTS.contains(&element.value().name()) {
        return String::new();
    }
    let mut text = VisibleText::default();
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
                    push_visible_text(&child_element, inside_struck_through, text);
                }
            }
            _ => {}
        }
    }
}

/// Joined text of an element, kept with and without its struck-through descendants.
#[derive(Default)]
struct VisibleText {
    without_struck_through: String,
    with_struck_through: String,
    after_dropped_text: bool,
}

impl VisibleText {
    fn push(&mut self, value: &str, struck_through: bool) {
        self.with_struck_through.push_str(value);
        if struck_through {
            self.after_dropped_text = true;
            return;
        }
        // `$25 <del>$50</del> ARS` must not leave the double space of the dropped amount.
        let value = if self.after_dropped_text
            && self.without_struck_through.ends_with(char::is_whitespace)
        {
            value.trim_start()
        } else {
            value
        };
        if !value.is_empty() {
            self.after_dropped_text = false;
        }
        self.without_struck_through.push_str(value);
    }

    fn into_value(self) -> String {
        if self.without_struck_through.trim().is_empty() {
            self.with_struck_through
        } else {
            self.without_struck_through
        }
    }
}

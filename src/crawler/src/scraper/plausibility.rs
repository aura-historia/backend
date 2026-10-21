//! Plausibility checks on raw extracted values. They inspect the strings a schema
//! extracted and never convert them into typed values.

/// Whether an extracted value still carries CSS rules or script source, as a selector
/// produces when it targets a container holding an inline `<style>` or `<script>` block.
pub(crate) fn contains_markup_tokens(value: &str) -> bool {
    contains_css_declaration_block(value)
        || value.contains("function(")
        || value.contains("function (")
        || value.contains("=>")
        || followed_by_identifier(value, "document.")
        || followed_by_identifier(value, "window.")
}

/// `{ ... }` holding a `property: value` declaration, so `.x{display:none}` counts while
/// prose braces such as `{rare}` do not.
fn contains_css_declaration_block(value: &str) -> bool {
    let mut rest = value;
    while let Some(open) = rest.find('{') {
        let after_open = &rest[open + 1..];
        let Some(close) = after_open.find('}') else {
            return false;
        };
        if after_open[..close].contains(':') {
            return true;
        }
        rest = &after_open[close + 1..];
    }
    false
}

/// `token` immediately followed by an identifier character, which is how script source
/// reads (`window.location`) and prose does not (`stained glass window. Circa 1880`).
fn followed_by_identifier(value: &str, token: &str) -> bool {
    value.match_indices(token).any(|(index, _)| {
        value[index + token.len()..]
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_alphabetic() || c == '_' || c == '$')
    })
}

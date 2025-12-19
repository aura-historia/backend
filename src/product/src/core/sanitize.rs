use html_escape::decode_html_entities; // decode HTML entities
use once_cell::sync::Lazy;
use regex::Regex;

static RE_HTML_TAG: Lazy<Regex> = Lazy::new(|| Regex::new(r"<[^>]+>").unwrap());
static RE_MULTIPLE_NEWLINES: Lazy<Regex> = Lazy::new(|| Regex::new(r"\n{3,}").unwrap());
// Detect numbered items even without space: "front2 Regalboden"
static RE_INLINE_NUMBERED: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"([^\n])(\d+)\s*([A-ZÄÖÜ])").unwrap());
// Break CamelCase-ish lists
static RE_CAMEL_LIST: Lazy<Regex> = Lazy::new(|| Regex::new(r"([a-zäöü])([A-ZÄÖÜ])").unwrap());
// Fix missing space after punctuation
static RE_PUNCT_SPACE: Lazy<Regex> = Lazy::new(|| Regex::new(r"([.!?])([A-Za-zÄÖÜäöü])").unwrap());
// Units (AFTER decimals are safe)
static RE_UNITS: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(\d+(?:,\d+)?)\s*(m|cm|mm|kg|g|l|inch|inches|lb)\b").unwrap());
static RE_SPACES: Lazy<Regex> = Lazy::new(|| Regex::new(r"[ \t]+").unwrap());
static RE_NEWLINE_SPACES: Lazy<Regex> = Lazy::new(|| Regex::new(r" *\n *").unwrap());

pub fn sanitize(input: &str) -> String {
    let text = decode_html_entities(input);

    // Remove lingering "&nbsp;" explicitly
    let text = text.replace("&nbsp;", "\n");

    // Strip HTML tags
    let text = RE_HTML_TAG.replace_all(&text, "");

    // Normalize newlines
    let text = text.replace("\r\n", "\n").replace('\r', "\n");
    let text = RE_MULTIPLE_NEWLINES.replace_all(&text, "\n\n");

    // Fix punctuation spacing early
    let text = RE_PUNCT_SPACE.replace_all(&text, "$1 $2");

    // Split inline numbered lists (KEEP numbers)
    let text = RE_INLINE_NUMBERED.replace_all(&text, "$1\n$2 $3");

    // Split CamelCase list items
    let text = RE_CAMEL_LIST.replace_all(&text, "$1\n$2");

    // Normalize units (preserve decimals)
    let text = RE_UNITS.replace_all(&text, "$1 $2");

    // Whitespace cleanup (newline-safe)
    let text = RE_SPACES.replace_all(&text, " ");
    let text = RE_NEWLINE_SPACES.replace_all(&text, "\n");

    text.trim().to_string()
}

#[cfg(test)]
mod tests {
    use crate::core::sanitize::sanitize;

    #[test]
    fn should_sanitize_description_like_text() {
        let s = "Diese zeitlos gestaltete Anrichte aus massiv Teakholz erhält ihre besondere Wirkung durch natürliche Unregelmäßigkeiten im Holz. Diese entstehen durch das verwendete Edelholz mit seinen verschiedenen Farben und Strukturen, welche diesem Stück einen eigentümlichen Charakter verleihen und es zu einem Unikat machen. Maserung und Farbton des Holzes tragen sehr zum optischen Wert dieses Möbels bei, das sowohl dekorative Möglichkeiten als auch genügend Stauraum bietet.&amp;nbsp;3 Schubladen2 Regalbodenmassiv TeakholzKorpus RAL 9010 lackiert / Oberplatte Natur&amp;nbsp;Maße:Breite 1,50 mHöhe 0,90 mTiefe 0,50 m&amp;nbsp;Bitte beachten Sie, dass es sich bei dem zweiten Foto um ein KI-generiertes Beispielfoto handelt, das lediglich der Veranschaulichung dient.";

        let actual = sanitize(s);

        assert!(
            actual.starts_with("Diese zeitlos gestaltete Anrichte aus massiv Teakholz erhält ihre")
        )
    }

    #[test]
    fn should_sanitize_title_like_text() {
        let s = "Anrichte Sideboard Teakholz 1,50 m";

        let expected = "Anrichte Sideboard Teakholz 1,50 m";
        let actual = sanitize(s);

        assert_eq!(expected, actual);
    }
}

//! Typography normalization for listing titles.
//!
//! Sources decorate titles with styled alphanumerics (`𝐄𝐢𝐧`, `Ⓥⓐⓢⓔ`, `ＶＡＳＥ`) and pictographs
//! (`🌿✨`). Styled letters and digits become their ordinary form, and pictographs, emoji
//! sequences and ornamental symbols are removed without joining the neighbouring words or
//! leaving a doubled or dangling separator. Every other character, including accented letters,
//! non-Latin scripts, punctuation, superscripts and `©`, `®` and `™`, is preserved.

use regex::{Regex, regex};
use unicode_normalization::UnicodeNormalization;

const EMOJI_PRESENTATION: char = '\u{FE0F}';
/// Separators that become dangling or doubled when a pictograph between them is removed.
const SEPARATORS: &[char] = &['|', '¦', '•', '·', '-', '–', '—', '/', ',', ';', ':'];
/// Punctuation that attaches to the preceding word.
const CLOSING: &[char] = &[',', '.', ';', ':', '!', '?', ')', ']', '}', '…'];
/// Punctuation that attaches to the following word.
const OPENING: &[char] = &['(', '[', '{'];

pub(super) fn normalize(text: &str) -> String {
    let plain = map_styled_alphanumerics(text);
    let without_pictographs = remove_pictographs(&plain);
    // Presentation selectors, keycap enclosures and tags left outside a removed pictograph
    // sequence are invisible; drop them without separating their neighbours.
    let without_components = regex!(r"[\x{20E3}\x{FE0E}\x{FE0F}\x{E0020}-\x{E007F}]")
        .replace_all(&without_pictographs, "");
    collapse_whitespace(&without_components)
}

/// Matches a run of pictographs together with their modifiers, presentation selectors, tags,
/// zero-width joiners and attached combining marks.
///
/// Pictographs are Unicode `Extended_Pictographic` characters, regional indicators, skin-tone
/// modifiers and the other symbols of the geometric shapes, miscellaneous symbols, dingbats,
/// miscellaneous symbols and arrows, and ornamental dingbats blocks, except punctuation such
/// as `‼` and the conventional `©`, `®` and `™` marks.
fn pictograph_run() -> &'static Regex {
    regex!(
        r"(?x)
        [\x{200D}\x{20E3}\x{FE0E}\x{FE0F}\x{E0020}-\x{E007F}]*
        (?:
            [
                \p{Extended_Pictographic}
                \p{Regional_Indicator}
                \x{1F3FB}-\x{1F3FF}
                [\p{So} && [\x{25A0}-\x{27BF} \x{2B00}-\x{2BFF} \x{1F650}-\x{1F67F}]]
                && [^\p{P} \x{A9} \x{AE} \x{2122}]
            ]
            [\p{M} \x{200D} \x{E0020}-\x{E007F}]*
        )+"
    )
}

fn is_pictograph(c: char) -> bool {
    pictograph_run().is_match(c.encode_utf8(&mut [0; 4]))
}

fn map_styled_alphanumerics(text: &str) -> String {
    let mut plain = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        // Letter-shaped emoji such as `🅰️` and `Ⓜ️` remain pictographs in emoji presentation.
        let emoji = chars.peek() == Some(&EMOJI_PRESENTATION) && is_pictograph(c);
        if !emoji && let Some(form) = plain_form(c) {
            plain.push_str(&form);
        } else {
            plain.push(c);
        }
    }
    plain
}

/// Returns the ordinary form of a styled letter or digit.
fn plain_form(c: char) -> Option<String> {
    let code = u32::from(c);
    match code {
        // Dingbat and double or negative circled digits have no compatibility decomposition.
        0x2776..=0x277F => Some((code - 0x2776 + 1).to_string()),
        0x2780..=0x2789 => Some((code - 0x2780 + 1).to_string()),
        0x278A..=0x2793 => Some((code - 0x278A + 1).to_string()),
        0x24EB..=0x24F4 => Some((code - 0x24EB + 11).to_string()),
        0x24F5..=0x24FE => Some((code - 0x24F5 + 1).to_string()),
        0x24FF | 0x1F10B | 0x1F10C => Some("0".to_owned()),
        // Negative circled and negative squared Latin capital letters.
        0x1F150..=0x1F169 => latin_capital(code - 0x1F150),
        0x1F170..=0x1F189 => latin_capital(code - 0x1F170),
        // Letterlike symbols also hold `™`, `℃` and `№`; only map styled single letters.
        0x2100..=0x214F => compatibility_form(c).filter(|form| {
            let mut chars = form.chars();
            chars.next().is_some_and(char::is_alphabetic) && chars.next().is_none()
        }),
        // Enclosed alphanumerics, mathematical alphanumerics and fullwidth letters and digits.
        0x2460..=0x24FF
        | 0x1D400..=0x1D7FF
        | 0x1F100..=0x1F1FF
        | 0xFF10..=0xFF19
        | 0xFF21..=0xFF3A
        | 0xFF41..=0xFF5A => compatibility_form(c),
        _ => None,
    }
}

fn latin_capital(offset: u32) -> Option<String> {
    char::from_u32(u32::from('A') + offset).map(String::from)
}

fn compatibility_form(c: char) -> Option<String> {
    let form: String = std::iter::once(c).nfkc().collect();
    form.chars().ne(std::iter::once(c)).then_some(form)
}

fn remove_pictographs(text: &str) -> String {
    let mut pieces = pictograph_run().split(text);
    let mut out = pieces.next().unwrap_or_default().to_owned();
    let mut trailing_gap = false;
    for piece in pieces {
        trailing_gap = !append_after_gap(&mut out, piece);
    }
    if trailing_gap {
        out.truncate(out.trim_end().len());
        if out.ends_with(SEPARATORS) {
            out.pop();
            out.truncate(out.trim_end().len());
        }
    }
    out
}

/// Appends the text following a removed pictograph run with a single word boundary.
/// Returns whether any text remained to append.
fn append_after_gap(out: &mut String, piece: &str) -> bool {
    out.truncate(out.trim_end().len());
    let mut piece = piece.trim_start();
    if out.is_empty() || out.ends_with(SEPARATORS) {
        piece = strip_leading_separator(piece);
    }
    if piece.is_empty() {
        return false;
    }
    if !out.is_empty() && !out.ends_with(OPENING) && !piece.starts_with(CLOSING) {
        out.push(' ');
    }
    out.push_str(piece);
    true
}

/// Strips a standalone leading separator such as `| `, but not the sign of `-20%`.
fn strip_leading_separator(piece: &str) -> &str {
    let mut chars = piece.chars();
    match chars.next() {
        Some(c)
            if SEPARATORS.contains(&c) && chars.clone().next().is_none_or(char::is_whitespace) =>
        {
            chars.as_str().trim_start()
        }
        _ => piece,
    }
}

fn collapse_whitespace(text: &str) -> String {
    let mut collapsed = String::with_capacity(text.len());
    for word in text.split_whitespace() {
        if !collapsed.is_empty() {
            collapsed.push(' ');
        }
        collapsed.push_str(word);
    }
    collapsed
}

#[cfg(test)]
mod tests {
    use super::normalize;
    use rstest::rstest;

    #[rstest]
    #[case::bold("𝐄𝐢𝐧 𝐌ö𝐛𝐞𝐥 𝟏𝟖𝟓𝟎", "Ein Möbel 1850")]
    #[case::italic_and_script("𝑉𝑎𝑠𝑒 𝓥𝓪𝓼𝓮", "Vase Vase")]
    #[case::fraktur_with_letterlike_capitals("ℭ𝔥𝔞𝔦𝔯 ℌ𝔬𝔩𝔷", "Chair Holz")]
    #[case::double_struck("𝕍𝕒𝕤𝕖 ℝ𝕠𝕥", "Vase Rot")]
    #[case::fullwidth("ＶＡＳＥ １８５０", "VASE 1850")]
    #[case::circled("Ⓥⓐⓢⓔ ⓂⒺⒾⓈⓈⒺⓃ", "Vase MEISSEN")]
    #[case::squared("🅅🄰🅂🄴 🆅🅰🆂🅴 🅥🅐🅢🅔", "VASE VASE VASE")]
    #[case::enclosed_digits("① ⑴ ⒈ ⓫ ⓵ ❶ ➀ ➊", "1 (1) 1. 11 1 1 1 1")]
    fn should_map_styled_alphanumerics(#[case] raw: &str, #[case] expected: &str) {
        assert_eq!(normalize(raw), expected);
    }

    #[rstest]
    #[case::glued_before_word("bringt 🌿✨Anrichte", "bringt Anrichte")]
    #[case::glued_between_words("Vase🌿Kommode", "Vase Kommode")]
    #[case::leading_and_trailing("✨ Vase ✨", "Vase")]
    #[case::only_pictographs("🌿✨", "")]
    #[case::zwj_sequence("Familie👨\u{200D}👩\u{200D}👧Figur", "Familie Figur")]
    #[case::flag_and_skin_tone("🇩🇪 Kommode 👍🏽", "Kommode")]
    #[case::subdivision_flag(
        "Teapot 🏴\u{E0067}\u{E0062}\u{E0065}\u{E006E}\u{E0067}\u{E007F}",
        "Teapot"
    )]
    #[case::emoji_presentation("Uhr ⌚\u{FE0F} rot ❤\u{FE0F}", "Uhr rot")]
    #[case::letter_shaped_emoji("🅰\u{FE0F} Ⓜ\u{FE0F} ℹ\u{FE0F} Info", "Info")]
    #[case::keycap("1\u{FE0F}\u{20E3} Stuhl", "1 Stuhl")]
    #[case::ornaments("★ Rarität ☆ ✿ ❀ ✦ ► ● ⚜", "Rarität")]
    #[case::doubled_separator("Vase | 🌿 | Kommode", "Vase | Kommode")]
    #[case::dangling_separators("🔥 | SALE | 🔥", "SALE")]
    #[case::closing_punctuation("Vase 🌿, rot 🌿!", "Vase, rot!")]
    #[case::separator_before_gap("Vase, 🌿 rot", "Vase, rot")]
    #[case::opening_punctuation("Vase (🌿 rot)", "Vase (rot)")]
    #[case::sign_after_gap("🔥 -20% Rabatt", "-20% Rabatt")]
    #[case::separated_runs("Vase 🌿 🌿 ✨ rot", "Vase rot")]
    fn should_remove_pictographs_without_joining_words(#[case] raw: &str, #[case] expected: &str) {
        assert_eq!(normalize(raw), expected);
    }

    #[rstest]
    #[case::german("Ölgemälde Möbel Größe Äpfel Übersee Straße")]
    #[case::latin_diacritics("Commode à la française – Æ Ø Å ç ñ ł ž ő")]
    #[case::other_scripts("Αρχαίο βάζο Антикварная ваза アンティーク花瓶 مزهرية عتيقة")]
    #[case::punctuation_and_identifiers(
        "Vase (19. Jh.) – rot/grün, Nr. 3360 & Co. #12 \"Original\" 50% ‼ ¿Qué? §4 † 1888"
    )]
    #[case::marks_and_units("Meissen® Porzellan ©1890 Marke™ 45° 2 m² ½ № 5 ℃ µm")]
    #[case::arrows_and_math("Länge → 120 cm × 80 cm ± 2")]
    fn should_preserve_ordinary_text(#[case] text: &str) {
        assert_eq!(normalize(text), text);
    }

    #[test]
    fn should_collapse_whitespace() {
        assert_eq!(
            normalize(" Vase \u{A0} rot\t\ngrün\u{3000}blau "),
            "Vase rot grün blau"
        );
    }
}

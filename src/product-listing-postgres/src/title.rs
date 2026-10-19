use product_listing_core::title::Title;

/// Decode canonical titles, titles stored before typography normalization and the bounded
/// trailing whitespace emitted by the old constructor, presenting their current canonical form.
/// Leading whitespace, case changes, punctuation changes and overlength values remain invalid.
pub(crate) fn decode_title(text: &str) -> Result<Title, ()> {
    let trimmed = text.trim_end();
    let legacy_trailing_whitespace = !trimmed.is_empty()
        && text.chars().count() <= Title::MAX_CHARS
        && Title::has_canonical_structure(trimmed);
    if Title::has_canonical_structure(text) || legacy_trailing_whitespace {
        Ok(Title::from(text))
    } else {
        Err(())
    }
}

/// Decode a stored title for readers that require title text. A title stored before typography
/// normalization with no text left, such as one made only of pictographs, is absent; a stored
/// empty title remains invalid.
pub(crate) fn decode_nonempty_title(text: &str) -> Result<Option<Title>, ()> {
    if text.is_empty() {
        return Err(());
    }
    let title = decode_title(text)?;
    Ok((!title.as_ref().is_empty()).then_some(title))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_canonical_and_legacy_constructor_output() {
        for text in [
            "Title",
            "Title...",
            "Title..",
            "Title ",
            "Title\t",
            "Title... ",
        ] {
            let title = decode_title(text).unwrap();
            assert_eq!(Title::from(title.as_ref()), title);
            assert_eq!(text.trim_end(), title.as_ref());
        }
        assert_eq!("", decode_title("").unwrap().as_ref());
    }

    #[test]
    fn presents_titles_stored_before_typography_normalization_canonically() {
        for (text, expected) in [
            (
                "𝐄𝐢𝐧 𝐌ö𝐛𝐞𝐥, 𝐝𝐚𝐬 𝐑𝐮𝐡𝐞 𝐢𝐧 𝐃𝐞𝐢𝐧 𝐙𝐮𝐡𝐚𝐮𝐬𝐞 𝐛𝐫𝐢𝐧𝐠𝐭 🌿✨Anrichte Biedermeier Kirschbaum | 3360",
                "Ein Möbel, das Ruhe in Dein Zuhause bringt Anrichte Biedermeier Kirschbaum | 3360",
            ),
            ("✨ vase", "Vase"),
            ("Vase  rot ✨ ", "Vase rot"),
            ("✨✨", ""),
        ] {
            let title = decode_title(text).unwrap();
            assert_eq!(expected, title.as_ref());
            assert_eq!(Title::from(title.as_ref()), title);
        }
    }

    #[test]
    fn presents_titles_without_text_as_absent_for_readers_requiring_text() {
        assert_eq!(
            "Vase",
            decode_nonempty_title("✨ vase").unwrap().unwrap().as_ref()
        );
        assert_eq!(None, decode_nonempty_title("✨✨").unwrap());
        assert!(decode_nonempty_title("").is_err());
        assert!(decode_nonempty_title("title").is_err());
    }

    #[test]
    fn rejects_other_noncanonical_persisted_titles() {
        for text in [
            "title".to_owned(),
            " Title".to_owned(),
            "Title.".to_owned(),
            "Title . ".to_owned(),
            " ".to_owned(),
            "𝐄𝐢𝐧 Möbel.".to_owned(),
            "A".repeat(129),
            format!("{} ", "A".repeat(128)),
        ] {
            assert!(
                decode_title(&text).is_err(),
                "unexpectedly accepted {text:?}"
            );
        }
    }
}

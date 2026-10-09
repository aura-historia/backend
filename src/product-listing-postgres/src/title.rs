use product_listing_core::title::Title;

/// Decode canonical titles and the bounded trailing whitespace emitted by the old constructor.
/// Leading whitespace, case changes, punctuation changes and overlength values remain invalid.
pub(crate) fn decode_title(text: &str) -> Result<Title, ()> {
    let title = Title::from(text);
    let legacy_trailing_whitespace = !title.as_ref().is_empty()
        && text.chars().count() <= Title::MAX_CHARS
        && title.as_ref() == text.trim_end();
    if title.as_ref() == text || legacy_trailing_whitespace {
        Ok(title)
    } else {
        Err(())
    }
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
    fn rejects_other_noncanonical_persisted_titles() {
        for text in [
            "title".to_owned(),
            " Title".to_owned(),
            "Title.".to_owned(),
            "Title . ".to_owned(),
            " ".to_owned(),
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

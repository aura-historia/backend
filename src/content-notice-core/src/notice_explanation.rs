use crate::bounded_text::{LAYOUT_CHARACTERS, bounded_text_newtype};

bounded_text_newtype!(
    /// The submitter's reasoned explanation: trimmed, 1–5000 characters, no control characters
    /// other than line breaks and tabs.
    NoticeExplanation,
    max_chars(5000),
    allowed_controls(LAYOUT_CHARACTERS)
);

#[cfg(test)]
mod tests {
    use super::NoticeExplanation;
    use crate::bounded_text::BoundedTextError;

    #[test]
    fn should_accept_explanation_at_length_bounds() {
        assert_eq!("x", NoticeExplanation::try_from("\nx\n").unwrap().as_str());
        let longest = "x".repeat(NoticeExplanation::MAX_CHARS);
        assert_eq!(
            longest,
            NoticeExplanation::try_from(longest.as_str())
                .unwrap()
                .as_str()
        );
    }

    #[test]
    fn should_reject_blank_or_too_long_explanation() {
        assert_eq!(
            Err(BoundedTextError::Blank),
            NoticeExplanation::try_from("\r\n\t ")
        );
        assert_eq!(
            Err(BoundedTextError::TooLong {
                max_chars: 5000,
                actual_chars: 5001,
            }),
            NoticeExplanation::try_from("x".repeat(5001))
        );
    }

    #[test]
    fn should_keep_line_breaks_and_tabs_but_reject_other_control_characters() {
        assert_eq!(
            "first line\r\n\tsecond line",
            NoticeExplanation::try_from("first line\r\n\tsecond line")
                .unwrap()
                .as_str()
        );
        for explanation in [
            "bell\u{7}!",
            "escape\u{1b}[0m",
            "delete\u{7f}!",
            "nel\u{85}!",
        ] {
            assert_eq!(
                Err(BoundedTextError::ControlCharacter),
                NoticeExplanation::try_from(explanation)
            );
        }
    }

    #[test]
    fn should_not_reveal_explanation_in_debug_output() {
        let explanation = NoticeExplanation::try_from("private detail").unwrap();

        assert!(!format!("{explanation:?}").contains("private"));
    }
}

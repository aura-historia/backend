use crate::adapter::EmbeddingAdapter;
use common::batch::Batch;
use product::core::{product::Product, product_event::ProductEventPayload};
use product_pipeline_common::process::{PipeProcessor, ProcessResult};
use std::{collections::HashSet, sync::Arc};
use tracing::{error, info};

/// Removes Markdown syntax from the given string while preserving all actual content.
fn remove_markdown(string: impl AsRef<str>) -> String {
    let input = string.as_ref();
    let mut result = String::with_capacity(input.len());
    let chars: Vec<char> = input.chars().collect();
    let len = chars.len();
    let mut i = 0;

    while i < len {
        let ch = chars[i];

        // Headings: lines starting with one or more '#' followed by space
        if ch == '#' && (i == 0 || chars[i - 1] == '\n') {
            while i < len && chars[i] == '#' {
                i += 1;
            }
            // skip optional space after hashes
            if i < len && chars[i] == ' ' {
                i += 1;
            }
            continue;
        }

        // Images: ![alt](url) - remove entirely (no textual content)
        if ch == '!' && i + 1 < len && chars[i + 1] == '[' {
            let alt_start = i + 2;
            if let Some(alt_end) = find_closing_bracket(&chars, alt_start)
                && alt_end + 1 < len
                && chars[alt_end + 1] == '('
                && let Some(url_end) = find_closing_paren(&chars, alt_end + 2)
            {
                i = url_end + 1;
                continue;
            }
        }

        // Links: [text](url) - keep text, remove url and syntax
        if ch == '[' {
            let text_start = i + 1;
            if let Some(text_end) = find_closing_bracket(&chars, text_start)
                && text_end + 1 < len
                && chars[text_end + 1] == '('
                && let Some(url_end) = find_closing_paren(&chars, text_end + 2)
            {
                let text: String = chars[text_start..text_end].iter().collect();
                result.push_str(&text);
                i = url_end + 1;
                continue;
            }
        }

        // Bold + italic: *** or ___
        if (ch == '*' || ch == '_') && i + 2 < len && chars[i + 1] == ch && chars[i + 2] == ch {
            i += 3;
            continue;
        }

        // Bold: ** or __
        if (ch == '*' || ch == '_') && i + 1 < len && chars[i + 1] == ch {
            i += 2;
            continue;
        }

        // Italic/emphasis: single * or _
        if ch == '*' || ch == '_' {
            i += 1;
            continue;
        }

        // Strikethrough: ~~
        if ch == '~' && i + 1 < len && chars[i + 1] == '~' {
            i += 2;
            continue;
        }

        // Inline code: `` (double backtick) or ` (single backtick) - keep content
        if ch == '`' {
            if i + 1 < len && chars[i + 1] == '`' {
                // Could be fenced code block (```) or double backtick
                if i + 2 < len && chars[i + 2] == '`' {
                    // Fenced code block: skip the ``` line
                    i += 3;
                    // Skip rest of the opening fence line (including the newline)
                    while i < len && chars[i] != '\n' {
                        i += 1;
                    }
                    if i < len && chars[i] == '\n' {
                        i += 1;
                    }
                    continue;
                }
                // Double backtick inline code: skip markers
                i += 2;
                continue;
            }
            // Single backtick: skip marker
            i += 1;
            continue;
        }

        // Blockquote: > at start of line
        if ch == '>' && (i == 0 || chars[i - 1] == '\n') {
            i += 1;
            if i < len && chars[i] == ' ' {
                i += 1;
            }
            continue;
        }

        // Horizontal rule: --- or *** or ___ (3+ chars on their own line)
        if (ch == '-' || ch == '*' || ch == '_')
            && (i == 0 || chars[i - 1] == '\n')
            && i + 2 < len
            && chars[i + 1] == ch
            && chars[i + 2] == ch
        {
            let mut j = i;
            while j < len && chars[j] == ch {
                j += 1;
            }
            if j >= len || chars[j] == '\n' {
                i = j;
                continue;
            }
        }

        // Unordered list markers: - , + , * at start of line followed by space
        if (ch == '-' || ch == '+')
            && (i == 0 || chars[i - 1] == '\n')
            && i + 1 < len
            && chars[i + 1] == ' '
        {
            i += 2;
            continue;
        }

        // Ordered list markers: digits followed by . or ) and space at start of line
        if ch.is_ascii_digit() && (i == 0 || chars[i - 1] == '\n') {
            let mut j = i;
            while j < len && chars[j].is_ascii_digit() {
                j += 1;
            }
            if j < len && (chars[j] == '.' || chars[j] == ')') && j + 1 < len && chars[j + 1] == ' '
            {
                i = j + 2;
                continue;
            }
        }

        result.push(ch);
        i += 1;
    }

    result
}

fn find_closing_bracket(chars: &[char], start: usize) -> Option<usize> {
    let mut i = start;
    while i < chars.len() {
        if chars[i] == ']' {
            return Some(i);
        }
        if chars[i] == '\n' {
            return None;
        }
        i += 1;
    }
    None
}

fn find_closing_paren(chars: &[char], start: usize) -> Option<usize> {
    let mut i = start;
    while i < chars.len() {
        if chars[i] == ')' {
            return Some(i);
        }
        if chars[i] == '\n' {
            return None;
        }
        i += 1;
    }
    None
}

pub struct TextEmbeddingPipeProcesserImpl {
    embedding_delegate: Arc<dyn EmbeddingAdapter + Send + Sync>,
}

impl TextEmbeddingPipeProcesserImpl {
    pub fn new(embedding_delegate: Arc<dyn EmbeddingAdapter + Send + Sync>) -> Self {
        Self { embedding_delegate }
    }
}

#[async_trait::async_trait]
impl PipeProcessor for TextEmbeddingPipeProcesserImpl {
    async fn process(&self, ins: Vec<Product>) -> ProcessResult {
        let count = ins.len();
        let mut successes = Vec::with_capacity(ins.len());
        let mut failures = HashSet::new();
        // Sort by text length so each batch contains items of similar length,
        // reducing padding overhead in transformer models.
        let mut sorted_ins = ins;
        sorted_ins.sort_by_key(|product| {
            product.native_title.payload.len()
                + product
                    .native_description
                    .as_ref()
                    .map_or(0, |d| d.payload.len())
        });
        let batches: Vec<Batch<Product, 64>> = Batch::chunked_from(sorted_ins.into_iter());

        for in_batch in batches {
            let input_batch_iter = in_batch.iter().map(|in_product| {
                let title = remove_markdown(in_product.native_title.payload.as_ref());
                let description = in_product
                    .native_description
                    .as_ref()
                    .map(|descr| remove_markdown(descr.payload.as_ref()))
                    .unwrap_or_default();
                format!("{title} [SEP] {description}")
            });
            let input_batch = Batch::try_from_iter(input_batch_iter)
                .expect("shouldn't fail re-collecting former batch of same size");

            match self.embedding_delegate.embed(&input_batch) {
                Err(err) => {
                    error!(error = %err, "Failed delegating embeddings.");
                    let mut local_failed = in_batch.iter().map(|in_product| in_product.product_id);
                    failures.extend(&mut local_failed);
                }
                Ok(embeddings) => {
                    let mut local_enriched = in_batch
                        .into_iter()
                        .zip(embeddings.into_iter())
                        .filter_map(|(mut product, embedding)| product.embed_text(embedding))
                        .map(|enrichment_event| {
                            enrichment_event.map_payload(ProductEventPayload::from)
                        });
                    successes.extend(&mut local_enriched);
                }
            }
        }

        info!(
            count = count,
            successes = count - failures.len(),
            failures = failures.len(),
            "Text-embedded translated products."
        );

        ProcessResult {
            successes,
            failures,
        }
    }
}

#[cfg(test)]
pub mod tests {
    use crate::{adapter::MockEmbeddingAdapter, process::TextEmbeddingPipeProcesserImpl};
    use product::core::product::Product;
    use product_pipeline_common::process::PipeProcessor;
    use pyo3::{PyErr, exceptions::PyTypeError};
    use rstest;
    use std::sync::Arc;

    #[tokio::test]
    async fn should_keep_order_of_delegate_returned_embeddings() {
        let expected = vec![
            vec![1.234, 5.6789],
            vec![1.234, -5.6789],
            vec![-1.234, 5.6789],
            vec![-1.234, -5.6789],
        ];
        let expected_clone = expected.clone();
        let mut delegate = MockEmbeddingAdapter::default();
        delegate
            .expect_embed()
            .return_once(move |_| Ok(expected_clone.try_into().unwrap()));

        let embedding_pipe = TextEmbeddingPipeProcesserImpl::new(Arc::new(delegate));
        let res = embedding_pipe.process(fake::vec![Product; 4]).await;
        let actual = res
            .successes
            .into_iter()
            .map(|event| {
                event
                    .payload
                    .as_enrichment_event()
                    .unwrap()
                    .as_embedded_text()
                    .unwrap()
                    .embedding
                    .clone()
            })
            .collect::<Vec<_>>();

        assert_eq!(expected, actual);
        assert!(res.failures.is_empty());
    }

    #[rstest::rstest]
    #[trace]
    #[case(1)]
    #[case(5)]
    #[case(20)]
    #[case(32)]
    #[case(64)]
    #[case(70)]
    #[case(128)]
    #[case(129)]
    #[case(256)]
    #[case(1000)]
    #[case(1500)]
    #[tokio::test]
    async fn should_partially_fail_entire_batches(#[case] input_count: usize) {
        let mut delegate = MockEmbeddingAdapter::default();
        delegate
            .expect_embed()
            .returning(move |_| Err(PyErr::new::<PyTypeError, _>("Something went wrong")));

        let embedding_pipe = TextEmbeddingPipeProcesserImpl::new(Arc::new(delegate));
        let res = embedding_pipe
            .process(fake::vec![Product; input_count])
            .await;

        assert!(res.successes.is_empty());
        assert_eq!(input_count, res.failures.len());
    }

    mod remove_markdown {
        use super::super::remove_markdown;
        use rstest::rstest;

        #[test]
        fn should_return_empty_string_when_input_is_empty() {
            assert_eq!(remove_markdown(""), "");
        }

        #[test]
        fn should_preserve_plain_text_when_no_markdown_present() {
            assert_eq!(
                remove_markdown("Hello, this is plain text."),
                "Hello, this is plain text."
            );
        }

        #[rstest]
        #[case("# Heading 1", "Heading 1")]
        #[case("## Heading 2", "Heading 2")]
        #[case("### Heading 3", "Heading 3")]
        #[case("#### Heading 4", "Heading 4")]
        #[case("##### Heading 5", "Heading 5")]
        #[case("###### Heading 6", "Heading 6")]
        fn should_remove_heading_markers_when_line_starts_with_hashes(
            #[case] input: &str,
            #[case] expected: &str,
        ) {
            assert_eq!(remove_markdown(input), expected);
        }

        #[test]
        fn should_remove_heading_markers_when_multiple_headings_on_separate_lines() {
            assert_eq!(
                remove_markdown("# Title\n## Subtitle\nContent"),
                "Title\nSubtitle\nContent"
            );
        }

        #[rstest]
        #[case("**bold**", "bold")]
        #[case("__bold__", "bold")]
        fn should_remove_bold_markers_when_text_is_bold(
            #[case] input: &str,
            #[case] expected: &str,
        ) {
            assert_eq!(remove_markdown(input), expected);
        }

        #[rstest]
        #[case("*italic*", "italic")]
        #[case("_italic_", "italic")]
        fn should_remove_italic_markers_when_text_is_italic(
            #[case] input: &str,
            #[case] expected: &str,
        ) {
            assert_eq!(remove_markdown(input), expected);
        }

        #[rstest]
        #[case("***bold italic***", "bold italic")]
        #[case("___bold italic___", "bold italic")]
        fn should_remove_bold_italic_markers_when_text_is_bold_italic(
            #[case] input: &str,
            #[case] expected: &str,
        ) {
            assert_eq!(remove_markdown(input), expected);
        }

        #[test]
        fn should_remove_strikethrough_markers_when_text_has_strikethrough() {
            assert_eq!(remove_markdown("~~deleted~~"), "deleted");
        }

        #[test]
        fn should_remove_inline_code_backticks_when_text_has_inline_code() {
            assert_eq!(remove_markdown("`code`"), "code");
        }

        #[test]
        fn should_remove_double_backtick_markers_when_text_has_double_backtick_code() {
            assert_eq!(remove_markdown("``code block``"), "code block");
        }

        #[test]
        fn should_remove_fenced_code_block_markers_when_text_has_code_block() {
            assert_eq!(remove_markdown("```rust\nlet x = 1;\n```"), "let x = 1;\n");
        }

        #[test]
        fn should_remove_fenced_code_block_markers_when_no_language_specified() {
            assert_eq!(remove_markdown("```\nsome code\n```"), "some code\n");
        }

        #[test]
        fn should_keep_link_text_when_removing_link_syntax() {
            assert_eq!(
                remove_markdown("[click here](https://example.com)"),
                "click here"
            );
        }

        #[test]
        fn should_remove_image_entirely_when_text_has_image() {
            assert_eq!(
                remove_markdown("![alt text](https://example.com/image.png)"),
                ""
            );
        }

        #[test]
        fn should_remove_blockquote_markers_when_line_starts_with_angle_bracket() {
            assert_eq!(remove_markdown("> quoted text"), "quoted text");
        }

        #[test]
        fn should_remove_nested_blockquote_markers_when_multiple_levels() {
            assert_eq!(
                remove_markdown("> first level\n> second line"),
                "first level\nsecond line"
            );
        }

        #[rstest]
        #[case("- item one", "item one")]
        #[case("+ item two", "item two")]
        fn should_remove_unordered_list_markers_when_line_starts_with_dash_or_plus(
            #[case] input: &str,
            #[case] expected: &str,
        ) {
            assert_eq!(remove_markdown(input), expected);
        }

        #[rstest]
        #[case("1. first item", "first item")]
        #[case("2. second item", "second item")]
        #[case("10. tenth item", "tenth item")]
        fn should_remove_ordered_list_markers_when_line_starts_with_number(
            #[case] input: &str,
            #[case] expected: &str,
        ) {
            assert_eq!(remove_markdown(input), expected);
        }

        #[rstest]
        #[case("1) first item", "first item")]
        #[case("2) second item", "second item")]
        fn should_remove_ordered_list_markers_when_using_parenthesis_style(
            #[case] input: &str,
            #[case] expected: &str,
        ) {
            assert_eq!(remove_markdown(input), expected);
        }

        #[rstest]
        #[case("---")]
        #[case("***")]
        #[case("___")]
        #[case("----")]
        fn should_remove_horizontal_rules_when_present(#[case] input: &str) {
            assert_eq!(remove_markdown(input), "");
        }

        #[test]
        fn should_handle_mixed_markdown_when_text_contains_multiple_elements() {
            let input = "# Welcome\n\nThis is **bold** and *italic* text.\n\n- item 1\n- item 2\n\n[link](https://example.com)";
            let expected = "Welcome\n\nThis is bold and italic text.\n\nitem 1\nitem 2\n\nlink";
            assert_eq!(remove_markdown(input), expected);
        }

        #[test]
        fn should_preserve_content_when_markdown_is_mixed_with_plain_text() {
            let input = "A product with **strong** features and *elegant* design";
            let expected = "A product with strong features and elegant design";
            assert_eq!(remove_markdown(input), expected);
        }

        #[test]
        fn should_preserve_numbers_in_text_when_not_list_markers() {
            assert_eq!(
                remove_markdown("There are 5 items in stock"),
                "There are 5 items in stock"
            );
        }

        #[test]
        fn should_accept_string_reference_when_called_with_string() {
            let input = String::from("**bold text**");
            assert_eq!(remove_markdown(&input), "bold text");
        }

        #[test]
        fn should_accept_owned_string_when_called_with_string() {
            let input = String::from("*italic*");
            assert_eq!(remove_markdown(input), "italic");
        }

        #[test]
        fn should_preserve_whitespace_when_text_has_spaces_and_newlines() {
            assert_eq!(
                remove_markdown("Hello   world\n\nNew paragraph"),
                "Hello   world\n\nNew paragraph"
            );
        }

        #[test]
        fn should_remove_link_syntax_when_link_appears_in_sentence() {
            assert_eq!(
                remove_markdown("Visit [our site](https://example.com) for details"),
                "Visit our site for details"
            );
        }

        #[test]
        fn should_remove_image_when_image_appears_in_sentence() {
            assert_eq!(remove_markdown("See ![photo](img.png) above"), "See  above");
        }

        #[test]
        fn should_preserve_hash_in_middle_of_line_when_not_heading() {
            assert_eq!(remove_markdown("Issue #42 is open"), "Issue #42 is open");
        }

        #[test]
        fn should_preserve_angle_bracket_in_middle_of_line_when_not_blockquote() {
            assert_eq!(remove_markdown("a > b"), "a > b");
        }

        #[test]
        fn should_preserve_dash_in_middle_of_line_when_not_list_marker() {
            assert_eq!(remove_markdown("well-known fact"), "well-known fact");
        }
    }
}

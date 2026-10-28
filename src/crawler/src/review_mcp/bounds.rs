//! Size bounds of the review tools' answers. An agent's context is finite and a tool answer above
//! the client's limit is dropped, so every answer stays below [`MAX_TOOL_OUTPUT_BYTES`] of JSON and
//! says what it left out: a shortened text ends in a `[truncated: ...]` marker, and a shortened
//! list, or anything [`bound_answer`] had to cut, is named in the answer's `truncations`.

use serde_json::Value;

/// The most bytes of JSON one tool answer holds, well below the default tool-result limit of
/// Claude Code.
pub(super) const MAX_TOOL_OUTPUT_BYTES: usize = 60 * 1024;
/// The key of the list in which an answer names what it left out.
const TRUNCATIONS: &str = "truncations";
/// How often [`bound_answer`] cuts before it gives up on an answer.
const MAX_CUTS: usize = 64;

/// `text` with at most `max_chars` characters; a longer one is cut and marked.
pub(super) fn truncated_text(text: &str, max_chars: usize) -> String {
    let total = text.chars().count();
    if total <= max_chars {
        return text.to_owned();
    }
    let kept: String = text.chars().take(max_chars).collect();
    format!("{kept}… [truncated: {max_chars} of {total} characters]")
}

/// The first `max` of `items`, naming the cut in `truncations` as `name`.
pub(super) fn keep_first<T>(
    mut items: Vec<T>,
    max: usize,
    name: &str,
    truncations: &mut Vec<String>,
) -> Vec<T> {
    if items.len() > max {
        truncations.push(format!("{name}: the first {max} of {}", items.len()));
        items.truncate(max);
    }
    items
}

/// The last `max` of `items`, naming the cut in `truncations` as `name`.
pub(super) fn keep_last<T>(
    mut items: Vec<T>,
    max: usize,
    name: &str,
    truncations: &mut Vec<String>,
) -> Vec<T> {
    if items.len() > max {
        truncations.push(format!("{name}: the last {max} of {}", items.len()));
        items.drain(..items.len() - max);
    }
    items
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
#[error("the answer is larger than {MAX_TOOL_OUTPUT_BYTES} bytes even after cutting it")]
pub(super) struct AnswerTooLarge;

/// Bounds a tool's JSON `answer` to `limit` bytes. An answer within it stays as it is. A larger one
/// that has a `truncations` list loses, one cut at a time, the second half of its largest list or
/// text, each cut named there by its JSON pointer, until it fits; the answer keeps its shape, so it
/// still matches the tool's output schema. Without that list, or when cutting does not make it fit,
/// it is refused.
pub(super) fn bound_answer(mut answer: Value, limit: usize) -> Result<Value, AnswerTooLarge> {
    let mut cuts = Vec::new();
    for _ in 0..MAX_CUTS {
        if json_len(&answer) + json_len(&Value::from(cuts.clone())) <= limit {
            return add_truncations(answer, cuts);
        }
        if answer.get(TRUNCATIONS).is_none() {
            return Err(AnswerTooLarge);
        }
        let Some((pointer, _)) = largest_cuttable(&answer, String::new()) else {
            return Err(AnswerTooLarge);
        };
        let Some(node) = answer.pointer_mut(&pointer) else {
            return Err(AnswerTooLarge);
        };
        cuts.push(cut_in_half(node, &pointer));
    }
    Err(AnswerTooLarge)
}

fn add_truncations(mut answer: Value, cuts: Vec<String>) -> Result<Value, AnswerTooLarge> {
    if cuts.is_empty() {
        return Ok(answer);
    }
    match answer.get_mut(TRUNCATIONS).and_then(Value::as_array_mut) {
        Some(truncations) => {
            truncations.extend(cuts.into_iter().map(Value::from));
            Ok(answer)
        }
        None => Err(AnswerTooLarge),
    }
}

fn json_len(value: &Value) -> usize {
    serde_json::to_vec(value).map_or(usize::MAX, |bytes| bytes.len())
}

/// The JSON pointer and size of the largest list with more than one item, or text of more than a
/// few characters, in `value`, leaving out the answer's own `truncations`.
fn largest_cuttable(value: &Value, pointer: String) -> Option<(String, usize)> {
    let own = match value {
        Value::Array(items) if items.len() > 1 => Some((pointer.clone(), json_len(value))),
        Value::String(text) if text.chars().count() > 64 => {
            Some((pointer.clone(), json_len(value)))
        }
        _ => None,
    };
    let children: Vec<(String, &Value)> = match value {
        Value::Array(items) => items
            .iter()
            .enumerate()
            .map(|(index, item)| (format!("{pointer}/{index}"), item))
            .collect(),
        Value::Object(fields) => fields
            .iter()
            .filter(|(key, _)| !(pointer.is_empty() && key.as_str() == TRUNCATIONS))
            .map(|(key, item)| (format!("{pointer}/{}", escape_pointer(key)), item))
            .collect(),
        _ => Vec::new(),
    };
    children
        .into_iter()
        .filter_map(|(child_pointer, child)| largest_cuttable(child, child_pointer))
        .chain(own)
        .max_by_key(|(_, size)| *size)
}

fn escape_pointer(key: &str) -> String {
    key.replace('~', "~0").replace('/', "~1")
}

/// Cuts the list or text at `pointer` to its first half and names the cut.
fn cut_in_half(node: &mut Value, pointer: &str) -> String {
    match node {
        Value::Array(items) => {
            let total = items.len();
            items.truncate(total.div_ceil(2));
            format!("{pointer}: the first {} of {total} items", items.len())
        }
        Value::String(text) => {
            let total = text.chars().count();
            let kept = total.div_ceil(2);
            *text = truncated_text(text, kept);
            format!("{pointer}: the first {kept} of {total} characters")
        }
        _ => format!("{pointer}: cut"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn should_mark_a_text_cut_to_its_bound() {
        assert_eq!(truncated_text("short", 10), "short");
        assert_eq!(
            truncated_text("äbcdefghij", 3),
            "äbc… [truncated: 3 of 10 characters]"
        );
    }

    #[test]
    fn should_name_the_list_items_left_out() {
        let mut truncations = Vec::new();

        let first = keep_first((1..=5).collect(), 2, "reviews", &mut truncations);
        let last = keep_last((1..=5).collect(), 2, "events", &mut truncations);
        let whole = keep_first(vec![1], 2, "pages", &mut truncations);

        assert_eq!(first, vec![1, 2]);
        assert_eq!(last, vec![4, 5]);
        assert_eq!(whole, vec![1]);
        assert_eq!(
            truncations,
            vec!["reviews: the first 2 of 5", "events: the last 2 of 5"]
        );
    }

    #[test]
    fn should_keep_an_answer_within_its_bound_as_it_is() {
        let answer = json!({ "matrix": { "pages": [1, 2, 3] }, "truncations": [] });

        assert_eq!(bound_answer(answer.clone(), 1_000), Ok(answer));
    }

    #[test]
    fn should_cut_the_largest_list_of_an_oversized_answer_and_name_each_cut() {
        let pages: Vec<Value> = (0..200)
            .map(|index| json!({ "url": format!("https://shop.example/item/{index}") }))
            .collect();
        let answer = json!({
            "review_id": "cr_1",
            "matrix": { "pages": pages, "schemas": [1, 2] },
            "truncations": ["events: the last 40 of 90"],
        });

        let bounded = bound_answer(answer, 2_000).unwrap_or_else(|error| panic!("{error}"));

        assert!(json_len(&bounded) <= 2_000, "{bounded}");
        assert_eq!(bounded["review_id"], "cr_1");
        assert_eq!(bounded["matrix"]["schemas"], json!([1, 2]));
        let kept = bounded["matrix"]["pages"].as_array().map_or(0, Vec::len);
        assert!(kept > 0 && kept < 200, "{kept}");
        let truncations: Vec<&str> = bounded["truncations"]
            .as_array()
            .map(|cuts| cuts.iter().filter_map(Value::as_str).collect())
            .unwrap_or_default();
        assert_eq!(truncations[0], "events: the last 40 of 90");
        assert!(
            truncations[1..]
                .iter()
                .all(|cut| cut.starts_with("/matrix/pages: the first ")),
            "{truncations:?}"
        );
    }

    #[test]
    fn should_cut_an_oversized_text_with_a_marker() {
        let answer = json!({ "projection": "x".repeat(5_000), "truncations": [] });

        let bounded = bound_answer(answer, 1_000).unwrap_or_else(|error| panic!("{error}"));

        let projection = bounded["projection"].as_str().unwrap_or_default();
        assert!(projection.contains("[truncated: "), "{projection}");
        assert!(json_len(&bounded) <= 1_000);
        assert_eq!(bounded["truncations"].as_array().map(Vec::len), Some(3));
    }

    #[test]
    fn should_refuse_an_oversized_answer_it_cannot_mark_or_cut() {
        assert_eq!(
            bound_answer(json!({ "pages": vec![1; 1_000] }), 100),
            Err(AnswerTooLarge)
        );
        assert_eq!(
            bound_answer(json!({ "id": "x", "truncations": [] }), 5),
            Err(AnswerTooLarge)
        );
    }
}

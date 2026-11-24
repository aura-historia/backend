//! Whether a listing's embedding revision changes what its current vector shows enough to embed it
//! again (docs/product-listing.md#embeddings).
//!
//! The vector shows the title, the description and the first image. A revision is compared with
//! the input the vector was computed from, never with the previous revision, so small changes
//! cannot pile up unnoticed and a return to the embedded input costs nothing.

use crate::ports::{EmbeddedProductInput, ImageHash, PendingProductEmbedding};
use std::collections::HashMap;
use unicode_normalization::UnicodeNormalization;

/// Titles at least this similar to the embedded title keep the vector. Titles are short and weigh
/// most in the embedding, so only small edits of longer titles pass.
const TITLE_SIMILARITY: f64 = 0.95;
/// Descriptions at least this similar to the embedded description keep the vector, such as one
/// whose text grew or shrank by about a fifth.
const DESCRIPTION_SIMILARITY: f64 = 0.9;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum EmbeddingInputChange {
    /// The current vector still shows the revision: it is confirmed without a provider call.
    Unchanged,
    /// The text is unchanged and the first image has a new URL: it is unchanged when the picture
    /// at the new URL matches the embedded one.
    NewImageUrl { embedded: ImageHash },
    /// The revision is embedded again.
    Changed,
}

/// Compares the pending revision with the input of the listing's current vector. Without a
/// recorded input, such as for a new listing, the revision is changed.
pub(super) fn embedding_input_change(pending: &PendingProductEmbedding) -> EmbeddingInputChange {
    let Some(embedded) = &pending.embedded else {
        return EmbeddingInputChange::Changed;
    };
    if embedded.source_event_id == pending.work.source_event_id {
        // The vector already stands for this revision.
        return EmbeddingInputChange::Unchanged;
    }
    if !text_unchanged(embedded, pending) {
        return EmbeddingInputChange::Changed;
    }
    match (&embedded.image, &pending.image_url) {
        (None, None) => EmbeddingInputChange::Unchanged,
        (Some(image), Some(url)) if image.url == *url => EmbeddingInputChange::Unchanged,
        (Some(image), Some(_)) => image
            .hash
            .map_or(EmbeddingInputChange::Changed, |embedded| {
                EmbeddingInputChange::NewImageUrl { embedded }
            }),
        // An image added, removed, or left out of the embedded input because it could not be
        // sent then.
        (None, Some(_)) | (Some(_), None) => EmbeddingInputChange::Changed,
    }
}

fn text_unchanged(embedded: &EmbeddedProductInput, pending: &PendingProductEmbedding) -> bool {
    similar(
        Some(&embedded.title),
        Some(pending.title.as_ref()),
        TITLE_SIMILARITY,
    ) && similar(
        embedded.description.as_deref(),
        pending.description.as_ref().map(AsRef::as_ref),
        DESCRIPTION_SIMILARITY,
    )
}

/// Whether the texts are equivalent after normalization or, with the same numbers, at least
/// `threshold` similar. An absent text equals an empty one. A changed, added or removed number,
/// such as a year, a count or a dimension, alters few characters but what the text says, so it is
/// always a change.
fn similar(embedded: Option<&str>, current: Option<&str>, threshold: f64) -> bool {
    let embedded = normalized(embedded.unwrap_or_default());
    let current = normalized(current.unwrap_or_default());
    if embedded == current {
        return true;
    }
    if embedded.is_empty() || current.is_empty() || numbers(&embedded) != numbers(&current) {
        return false;
    }
    similarity(&embedded, &current) >= threshold
}

/// The text's runs of digits, in any script, in order.
fn numbers(text: &str) -> Vec<&str> {
    text.split(|character: char| !character.is_numeric())
        .filter(|number| !number.is_empty())
        .collect()
}

/// The text without differences an embedding does not see: Unicode compatibility forms, case,
/// punctuation, symbols and whitespace. Words stay separated by single spaces.
fn normalized(text: &str) -> String {
    let mut normalized = String::with_capacity(text.len());
    for word in text
        .nfkc()
        .flat_map(char::to_lowercase)
        .map(|character| {
            if character.is_alphanumeric() {
                character
            } else {
                ' '
            }
        })
        .collect::<String>()
        .split_whitespace()
    {
        if !normalized.is_empty() {
            normalized.push(' ');
        }
        normalized.push_str(word);
    }
    normalized
}

/// The Dice coefficient of the texts' character trigrams, counted with multiplicity: 1 for equal
/// texts, about `2 / (2 + share)` for a text that gained `share` of its length. Linear in the
/// texts' length, unlike an edit distance, so long descriptions stay cheap.
fn similarity(left: &str, right: &str) -> f64 {
    let left = trigrams(left);
    let right = trigrams(right);
    let total = left.values().sum::<usize>() + right.values().sum::<usize>();
    let shared = left
        .iter()
        .map(|(trigram, count)| right.get(trigram).map_or(0, |other| (*count).min(*other)))
        .sum::<usize>();
    (2 * shared) as f64 / total as f64
}

/// The text's character trigrams; a text of fewer than three characters is its own trigram.
fn trigrams(text: &str) -> HashMap<[char; 3], usize> {
    let characters = text.chars().collect::<Vec<_>>();
    let mut trigrams = HashMap::new();
    if characters.len() < 3 {
        let mut padded = ['\0'; 3];
        padded[..characters.len()].copy_from_slice(&characters);
        trigrams.insert(padded, 1);
        return trigrams;
    }
    for window in characters.windows(3) {
        *trigrams
            .entry([window[0], window[1], window[2]])
            .or_insert(0) += 1;
    }
    trigrams
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ports::{EmbeddedProductImage, ProductEmbeddingWorkRef};
    use domain_primitives::event_id::EventId;
    use product_listing_core::{
        description::Description, product_listing_id::ProductListingId, title::Title,
    };
    use url::Url;

    const DESCRIPTION: &str = "Bemalter Bauernschrank aus Weichholz, um 1850, mit zwei Türen, \
        originaler Bemalung und geschmiedeten Beschlägen. Altersgemäße Gebrauchsspuren.";

    fn url(path: &str) -> Url {
        Url::parse(&format!("https://shop.example{path}")).unwrap()
    }

    fn hash(seed: u8) -> ImageHash {
        ImageHash::from_bytes(&[seed; ImageHash::BYTES]).unwrap()
    }

    fn embedded() -> EmbeddedProductInput {
        EmbeddedProductInput {
            source_event_id: EventId::new(),
            title: "Bauernschrank, um 1850".to_owned(),
            description: Some(DESCRIPTION.to_owned()),
            image: Some(EmbeddedProductImage {
                url: url("/schrank.jpg?v=1"),
                hash: Some(hash(0)),
            }),
        }
    }

    /// The pending revision with the embedded input unchanged.
    fn pending() -> PendingProductEmbedding {
        PendingProductEmbedding {
            work: ProductEmbeddingWorkRef {
                product_listing_id: ProductListingId::new(),
                source_event_id: EventId::new(),
            },
            title: Title::from("Bauernschrank, um 1850"),
            description: Some(Description::from(DESCRIPTION)),
            image_url: Some(url("/schrank.jpg?v=1")),
            attempts: 0,
            embedded: Some(embedded()),
        }
    }

    fn change(edit: impl FnOnce(&mut PendingProductEmbedding)) -> EmbeddingInputChange {
        let mut pending = pending();
        edit(&mut pending);
        embedding_input_change(&pending)
    }

    #[test]
    fn should_embed_a_listing_without_a_recorded_input() {
        assert_eq!(
            EmbeddingInputChange::Changed,
            change(|pending| pending.embedded = None)
        );
    }

    #[test]
    fn should_keep_the_vector_for_text_equivalent_after_normalization() {
        assert_eq!(EmbeddingInputChange::Unchanged, change(|_| {}));
        assert_eq!(
            EmbeddingInputChange::Unchanged,
            change(|pending| {
                pending.title = Title::from("BAUERNSCHRANK – um\u{a0}1850!");
                pending.description = Some(Description::from(
                    DESCRIPTION
                        .replace(", ", " ; ")
                        .replace('ä', "a\u{308}")
                        .as_str(),
                ));
            })
        );
        assert_eq!(
            EmbeddingInputChange::Unchanged,
            change(|pending| pending.title = Title::from("Ｂａｕｅｒｎｓｃｈｒａｎｋ um 1850")),
            "fullwidth compatibility forms"
        );
    }

    #[test]
    fn should_embed_again_for_a_meaningfully_different_title() {
        for title in [
            "Bauernschrank, um 1880",
            "Bauernschrank, um 1850 - Antiquitäten Müller - Schränke",
            "Kommode",
        ] {
            assert_eq!(
                EmbeddingInputChange::Changed,
                change(|pending| pending.title = Title::from(title)),
                "{title}"
            );
        }
    }

    #[test]
    fn should_keep_the_vector_when_a_small_share_of_the_description_changes() {
        assert_eq!(
            EmbeddingInputChange::Unchanged,
            change(|pending| {
                pending.description = Some(Description::from(
                    format!("{DESCRIPTION} Versand möglich.").as_str(),
                ));
            }),
            "a short addition"
        );
        assert_eq!(
            EmbeddingInputChange::Unchanged,
            change(|pending| {
                pending.description = Some(Description::from(
                    DESCRIPTION.replace(" mit zwei Türen,", "").as_str(),
                ));
            }),
            "a short removal"
        );
    }

    #[test]
    fn should_embed_again_when_a_large_share_of_the_description_changes() {
        for description in [
            Some(format!("{DESCRIPTION} {DESCRIPTION}")),
            Some("Schrank.".to_owned()),
            None,
        ] {
            assert_eq!(
                EmbeddingInputChange::Changed,
                change(|pending| {
                    pending.description = description.as_deref().map(Description::from);
                }),
                "{description:?}"
            );
        }
        assert_eq!(
            EmbeddingInputChange::Changed,
            change(|pending| {
                pending.embedded.as_mut().unwrap().description = None;
            }),
            "a description set"
        );
    }

    #[test]
    fn should_embed_again_when_a_number_changes_in_a_long_text() {
        let long_title = "Bemalter Bauernschrank aus Weichholz mit zwei Türen, Tirol, um 1850";
        let embedded_long_title = |pending: &mut PendingProductEmbedding| {
            pending.embedded.as_mut().unwrap().title = long_title.to_owned();
        };
        assert_eq!(
            EmbeddingInputChange::Unchanged,
            change(|pending| {
                embedded_long_title(pending);
                pending.title = Title::from(long_title.replace("Bemalter", "Bemalte").as_str());
            }),
            "a small edit of a long title"
        );
        assert_eq!(
            EmbeddingInputChange::Changed,
            change(|pending| {
                embedded_long_title(pending);
                pending.title = Title::from(long_title.replace("1850", "1880").as_str());
            })
        );
        assert_eq!(
            EmbeddingInputChange::Changed,
            change(|pending| {
                pending.description = Some(Description::from(
                    DESCRIPTION.replace("um 1850", "um 1860").as_str(),
                ));
            }),
            "a description is as strict about numbers"
        );
    }

    #[test]
    fn should_keep_the_vector_of_a_revision_it_already_stands_for() {
        assert_eq!(
            EmbeddingInputChange::Unchanged,
            change(|pending| {
                pending.work.source_event_id = pending.embedded.as_ref().unwrap().source_event_id;
                pending.image_url = None;
            })
        );
    }

    #[test]
    fn should_measure_text_against_the_embedded_input_not_the_previous_revision() {
        // Each step from the previous title is small; together they leave the embedded one.
        assert_eq!(
            EmbeddingInputChange::Changed,
            change(|pending| {
                pending.title = Title::from("Bauernschrank aus Tirol, um 1850, Weichholz");
            })
        );
    }

    #[test]
    fn should_keep_the_vector_for_the_same_image_url_and_compare_a_new_url() {
        assert_eq!(
            EmbeddingInputChange::NewImageUrl { embedded: hash(0) },
            change(|pending| pending.image_url = Some(url("/schrank.jpg?v=2")))
        );
        assert_eq!(
            EmbeddingInputChange::Changed,
            change(|pending| {
                pending.image_url = Some(url("/schrank.heic"));
                pending
                    .embedded
                    .as_mut()
                    .unwrap()
                    .image
                    .as_mut()
                    .unwrap()
                    .hash = None;
            }),
            "an embedded image without a hash cannot be compared"
        );
    }

    #[test]
    fn should_embed_again_when_the_first_image_is_added_or_removed() {
        assert_eq!(
            EmbeddingInputChange::Changed,
            change(|pending| pending.image_url = None)
        );
        assert_eq!(
            EmbeddingInputChange::Changed,
            change(|pending| pending.embedded.as_mut().unwrap().image = None),
            "an image added, or one the embedding could not send then"
        );
        assert_eq!(
            EmbeddingInputChange::Unchanged,
            change(|pending| {
                pending.image_url = None;
                pending.embedded.as_mut().unwrap().image = None;
            })
        );
    }

    #[test]
    fn should_score_additions_by_their_share_of_the_text() {
        let text = normalized(DESCRIPTION);
        let tenth = text.chars().take(text.chars().count() / 10);
        let grown = format!("{text} {}", tenth.collect::<String>());

        assert_eq!(1.0, similarity(&text, &text));
        assert!(similarity(&text, &grown) > 0.9);
        assert!(similarity("ab", "abc") < 0.9);
    }
}

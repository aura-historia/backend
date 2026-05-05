//! Regenerate Gemini embeddings for every category and period in the seed data files.
//!
//! Reads `data/categories.json` and `data/periods.json`, calls the Gemini
//! embedding API for the combined meta-fields of each entry, and writes the
//! updated JSON (with fresh `embedding` arrays) back to the same paths,
//! overwriting the previous values.
//!
//! # Usage
//!
//! ```bash
//! GEMINI_API_KEY=<your-key> cargo run --bin hydrate_data -p product-classification
//! ```
//!
//! # Embedding text
//!
//! For each entry the text passed to the Gemini API is:
//! ```
//! title: {metaName} | text: {metaDescription} {keyword1}, {keyword2}, ...
//! ```
//! This matches the format produced by `Category::embedding_text()` and
//! `Period::embedding_text()` in the domain layer so that query and document
//! embeddings live in the same vector space.
//!
//! The `RETRIEVAL_DOCUMENT` task type is used as recommended by the Gemini
//! documentation for document-side embeddings.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::Path;

// ---------------------------------------------------------------------------
// Gemini API types
// ---------------------------------------------------------------------------

#[derive(Serialize)]
struct EmbedRequest<'a> {
    model: &'a str,
    content: EmbedContent<'a>,
    #[serde(rename = "taskType")]
    task_type: &'a str,
}

#[derive(Serialize)]
struct EmbedContent<'a> {
    parts: Vec<EmbedPart<'a>>,
}

#[derive(Serialize)]
struct EmbedPart<'a> {
    text: &'a str,
}

#[derive(Deserialize)]
struct EmbedResponse {
    embedding: EmbedValues,
}

#[derive(Deserialize)]
struct EmbedValues {
    values: Vec<f32>,
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Build the embedding input string from the meta fields — identical to the
/// format used by `Category::embedding_text()` and `Period::embedding_text()`.
fn build_embedding_text(
    meta_name: &str,
    meta_description: &str,
    meta_keywords: &[String],
) -> String {
    format!(
        "title: {} | text: {} {}",
        meta_name,
        meta_description,
        meta_keywords.join(", ")
    )
}

/// Call the Gemini embedding API and return the normalised 768-dimensional vector.
async fn embed(
    client: &reqwest::Client,
    api_key: &str,
    text: &str,
) -> Result<Vec<f32>, Box<dyn std::error::Error>> {
    let request = EmbedRequest {
        model: "models/gemini-embedding-2",
        content: EmbedContent {
            parts: vec![EmbedPart { text }],
        },
        task_type: "RETRIEVAL_DOCUMENT",
    };

    let response = client
        .post("https://generativelanguage.googleapis.com/v1beta/models/gemini-embedding-2:embedContent")
        .header("x-goog-api-key", api_key)
        .query(&[("output_dimensionality", "768")])
        .json(&request)
        .send()
        .await?
        .error_for_status()?;

    let body: EmbedResponse = response.json().await?;
    let mut values = body.embedding.values;

    if values.is_empty() {
        return Err("Gemini returned an empty embedding vector".into());
    }

    // Normalise to unit length — matching the behaviour of `MultimodalEmbeddingServiceImpl`.
    let norm: f32 = values.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm > 0.0 {
        for v in &mut values {
            *v /= norm;
        }
    }

    Ok(values)
}

/// Process a JSON array (loaded from one of the seed files), regenerating the
/// `embedding` field for every entry.  Returns the total number of entries
/// updated.
async fn hydrate_entries(
    client: &reqwest::Client,
    api_key: &str,
    entries: &mut [Value],
    id_field: &str,
) -> Result<usize, Box<dyn std::error::Error>> {
    let total = entries.len();
    for (i, entry) in entries.iter_mut().enumerate() {
        let id = entry[id_field].as_str().unwrap_or("<unknown>").to_string();

        let meta_name = entry["metaName"]
            .as_str()
            .ok_or_else(|| format!("missing metaName for entry {id}"))?
            .to_string();
        let meta_description = entry["metaDescription"]
            .as_str()
            .ok_or_else(|| format!("missing metaDescription for entry {id}"))?
            .to_string();
        let meta_keywords: Vec<String> = entry["metaKeywords"]
            .as_array()
            .unwrap_or(&Vec::new())
            .iter()
            .filter_map(|v| v.as_str().map(str::to_owned))
            .collect();

        let text = build_embedding_text(&meta_name, &meta_description, &meta_keywords);

        println!("[{}/{}] Embedding '{id}' …", i + 1, total);

        let embedding = embed(client, api_key, &text).await?;

        entry["embedding"] = serde_json::to_value(&embedding)?;
    }

    Ok(total)
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let api_key =
        std::env::var("GEMINI_API_KEY").expect("GEMINI_API_KEY environment variable must be set");

    let workspace_dir = env!("CARGO_WORKSPACE_DIR");
    let categories_path =
        Path::new(workspace_dir).join("src/product-classification/data/categories.json");
    let periods_path =
        Path::new(workspace_dir).join("src/product-classification/data/periods.json");

    let client = reqwest::Client::new();

    // --- categories ---
    println!(
        "=== Hydrating categories ({}) ===",
        categories_path.display()
    );
    let categories_raw = std::fs::read_to_string(&categories_path)?;
    let mut categories: Vec<Value> = serde_json::from_str(&categories_raw)?;
    let n_categories = hydrate_entries(&client, &api_key, &mut categories, "categoryId").await?;
    let categories_out = serde_json::to_string_pretty(&categories)?;
    std::fs::write(&categories_path, categories_out)?;
    println!(
        "✓ Wrote {n_categories} updated categories to {}",
        categories_path.display()
    );

    // --- periods ---
    println!("\n=== Hydrating periods ({}) ===", periods_path.display());
    let periods_raw = std::fs::read_to_string(&periods_path)?;
    let mut periods: Vec<Value> = serde_json::from_str(&periods_raw)?;
    let n_periods = hydrate_entries(&client, &api_key, &mut periods, "periodId").await?;
    let periods_out = serde_json::to_string_pretty(&periods)?;
    std::fs::write(&periods_path, periods_out)?;
    println!(
        "✓ Wrote {n_periods} updated periods to {}",
        periods_path.display()
    );

    println!("\nDone. {n_categories} categories and {n_periods} periods refreshed.");
    Ok(())
}

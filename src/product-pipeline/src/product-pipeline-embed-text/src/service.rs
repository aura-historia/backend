use async_trait::async_trait;
use base64::{Engine, engine::general_purpose::STANDARD as BASE64};
use common::logging::{
    LlmInvocationMetrics, LlmModel, LlmOperation, LlmProvider, log_llm_invocation,
};
use product::core::{description::Description, title::Title};
use serde::{Deserialize, Serialize};
use std::time::Instant;
use thiserror::Error;
use tracing::{debug, warn};
use url::Url;

#[derive(Debug, Error)]
pub enum MultimodalEmbeddingError {
    #[error("Gemini API request failed: {0}")]
    RequestFailed(#[from] reqwest::Error),
    #[error("Gemini API returned error: {0}")]
    ApiError(String),
    #[error("Empty embedding response from Gemini API")]
    EmptyResponse,
}

#[async_trait]
#[mockall::automock]
pub trait MultimodalEmbeddingService {
    async fn embed(
        &self,
        title: &Title,
        description: Option<&Description>,
        image: Option<&Url>,
    ) -> Result<Vec<f32>, MultimodalEmbeddingError>;

    /// Embed a free-text product search query.
    ///
    /// Uses the Gemini `RETRIEVAL_QUERY` task type as recommended in
    /// <https://ai.google.dev/gemini-api/docs/embeddings#task-types-embeddings-2>
    /// so the resulting vector lives in the same space as documents embedded with
    /// `RETRIEVAL_DOCUMENT` (or its multimodal equivalent used in [`Self::embed`]).
    async fn embed_query(&self, query: &str) -> Result<Vec<f32>, MultimodalEmbeddingError>;
}

/// Maximum number of cached `embed_query` results held in memory by an
/// [`MultimodalEmbeddingServiceImpl`]. 4096 entries × 768 f32 ≈ 12 MB worst case —
/// bounded so the warm Lambda stays lightweight while still amortising query embedding
/// cost across paged calls and warm invocations.
const QUERY_EMBEDDING_CACHE_CAPACITY: usize = 4096;

pub struct MultimodalEmbeddingServiceImpl {
    api_key: String,
    client: reqwest::Client,
    /// LRU cache of `embed_query` results, keyed by raw query string. Only `embed_query`
    /// uses this cache — `embed` (multimodal product ingestion) is one-shot per product
    /// event and deduplication should happen upstream.
    query_cache: tokio::sync::Mutex<lru::LruCache<String, Vec<f32>>>,
}

impl MultimodalEmbeddingServiceImpl {
    pub fn new(api_key: &str) -> Self {
        Self {
            api_key: api_key.to_string(),
            client: reqwest::Client::new(),
            query_cache: tokio::sync::Mutex::new(lru::LruCache::new(
                std::num::NonZeroUsize::new(QUERY_EMBEDDING_CACHE_CAPACITY)
                    .expect("QUERY_EMBEDDING_CACHE_CAPACITY must be non-zero"),
            )),
        }
    }

    fn build_content_parts(
        title: &Title,
        description: Option<&Description>,
        image_data: Option<(String, String)>,
    ) -> Vec<ContentPart> {
        let mut parts = Vec::with_capacity(3);

        // formatting according to official guidelines
        // https://ai.google.dev/gemini-api/docs/embeddings#task-types-embeddings-2
        let text = format!(
            "title: {title} | text: {}",
            description
                .map(Description::to_string)
                .unwrap_or("none".into())
        );
        parts.push(ContentPart::Text { text });

        if let Some((mime_type, data)) = image_data {
            parts.push(ContentPart::InlineData {
                inline_data: InlineData { mime_type, data },
            });
        }

        parts
    }

    async fn fetch_image(&self, url: &Url) -> Option<(String, String)> {
        match self.client.get(url.as_str()).send().await {
            Ok(response) => {
                let content_type = response
                    .headers()
                    .get(reqwest::header::CONTENT_TYPE)
                    .and_then(|v| v.to_str().ok())
                    .unwrap_or("image/jpeg")
                    .to_string();

                let mime_type = content_type
                    .split(';')
                    .next()
                    .unwrap_or("image/jpeg")
                    .trim()
                    .to_string();

                match response.bytes().await {
                    Ok(bytes) => {
                        let encoded = BASE64.encode(&bytes);
                        Some((mime_type, encoded))
                    }
                    Err(err) => {
                        warn!(error = %err, url = %url, "Failed reading image bytes.");
                        None
                    }
                }
            }
            Err(err) => {
                warn!(error = %err, url = %url, "Failed fetching image.");
                None
            }
        }
    }
}

#[async_trait]
impl MultimodalEmbeddingService for MultimodalEmbeddingServiceImpl {
    async fn embed(
        &self,
        title: &Title,
        description: Option<&Description>,
        image: Option<&Url>,
    ) -> Result<Vec<f32>, MultimodalEmbeddingError> {
        let image_data = match image {
            Some(url) => self.fetch_image(url).await,
            None => None,
        };

        let parts = Self::build_content_parts(title, description, image_data);

        let request = EmbedContentRequest {
            model: "models/gemini-embedding-2",
            content: Content { parts },
            task_type: None,
        };

        debug!("Requesting multimodal embedding from Gemini API.");

        let started_at = Instant::now();
        let response = self
            .client
            .post("https://generativelanguage.googleapis.com/v1beta/models/gemini-embedding-2:embedContent")
            .header("x-goog-api-key", &self.api_key)
            .query(&[("output_dimensionality", "768")])
            .json(&request)
            .send()
            .await?
            .error_for_status()
            .map_err(MultimodalEmbeddingError::RequestFailed)?;

        let body: EmbedContentResponse = response.json().await?;
        // normalize the embedding vector to unit length
        let mut values = body.embedding.values;
        if values.is_empty() {
            return Err(MultimodalEmbeddingError::EmptyResponse);
        }
        let norm = values.iter().map(|x| x * x).sum::<f32>().sqrt();
        if norm == 0.0 {
            return Err(MultimodalEmbeddingError::EmptyResponse);
        }
        for v in &mut values {
            *v /= norm;
        }

        log_llm_invocation(
            LlmOperation::ProductEmbedding,
            LlmProvider::Google,
            LlmModel::GeminiEmbedding2,
            started_at.elapsed(),
            LlmInvocationMetrics {
                output_dimensions: Some(values.len()),
                cache_hit: Some(false),
                ..Default::default()
            },
        );

        Ok(values)
    }

    async fn embed_query(&self, query: &str) -> Result<Vec<f32>, MultimodalEmbeddingError> {
        // Cache hit: serve from the in-memory LRU. The cache is mutated on `get` (LRU
        // promotion) so we need a `&mut` lock here; the lock is held only for the lookup.
        if let Some(hit) = self.query_cache.lock().await.get(query).cloned() {
            log_llm_invocation(
                LlmOperation::ProductQueryEmbedding,
                LlmProvider::Google,
                LlmModel::GeminiEmbedding2,
                std::time::Duration::default(),
                LlmInvocationMetrics {
                    output_dimensions: Some(hit.len()),
                    cache_hit: Some(true),
                    ..Default::default()
                },
            );
            return Ok(hit);
        }

        let request = EmbedContentRequest {
            model: "models/gemini-embedding-2",
            content: Content {
                parts: vec![ContentPart::Text {
                    text: query.to_string(),
                }],
            },
            task_type: Some("RETRIEVAL_QUERY"),
        };

        debug!("Requesting query embedding from Gemini API.");

        let started_at = Instant::now();
        let response = self
            .client
            .post("https://generativelanguage.googleapis.com/v1beta/models/gemini-embedding-2:embedContent")
            .header("x-goog-api-key", &self.api_key)
            .query(&[("output_dimensionality", "768")])
            .json(&request)
            .send()
            .await?
            .error_for_status()
            .map_err(MultimodalEmbeddingError::RequestFailed)?;

        let body: EmbedContentResponse = response.json().await?;
        let mut values = body.embedding.values;
        if values.is_empty() {
            return Err(MultimodalEmbeddingError::EmptyResponse);
        }
        let norm = values.iter().map(|x| x * x).sum::<f32>().sqrt();
        if norm == 0.0 {
            return Err(MultimodalEmbeddingError::EmptyResponse);
        }
        for v in &mut values {
            *v /= norm;
        }

        // Populate the cache with the freshly computed embedding for subsequent calls.
        self.query_cache
            .lock()
            .await
            .put(query.to_string(), values.clone());

        log_llm_invocation(
            LlmOperation::ProductQueryEmbedding,
            LlmProvider::Google,
            LlmModel::GeminiEmbedding2,
            started_at.elapsed(),
            LlmInvocationMetrics {
                output_dimensions: Some(values.len()),
                cache_hit: Some(false),
                ..Default::default()
            },
        );

        Ok(values)
    }
}

#[derive(Debug, Serialize)]
struct EmbedContentRequest<'a> {
    model: &'a str,
    content: Content,
    #[serde(rename = "taskType", skip_serializing_if = "Option::is_none")]
    task_type: Option<&'a str>,
}

#[derive(Debug, Serialize)]
struct Content {
    parts: Vec<ContentPart>,
}

#[derive(Debug, Serialize)]
#[serde(untagged)]
enum ContentPart {
    Text {
        text: String,
    },
    InlineData {
        #[serde(rename = "inlineData")]
        inline_data: InlineData,
    },
}

#[derive(Debug, Serialize)]
struct InlineData {
    #[serde(rename = "mimeType")]
    mime_type: String,
    data: String,
}

#[derive(Debug, Deserialize)]
struct EmbedContentResponse {
    embedding: Embedding,
}

#[derive(Debug, Deserialize)]
struct Embedding {
    values: Vec<f32>,
}

#[cfg(test)]
mod tests {
    use super::*;

    const GEMINI_EXAMPLE_RESPONSE_EMBEDDING_768: [f32; 768] = [
        -0.036270842,
        0.02361682,
        -0.0029220004,
        -0.016072785,
        0.02316376,
        0.008332699,
        -0.02891746,
        0.015677461,
        -0.01463142,
        -0.10077077,
        0.029492525,
        0.02435133,
        0.04219972,
        -0.014070857,
        0.0025885715,
        0.015626293,
        -0.02128292,
        -0.016839612,
        -0.033849,
        -0.005133642,
        -0.015667764,
        -0.022695456,
        -0.0026581238,
        0.004976106,
        -0.06931419,
        -0.0021109623,
        -0.021948576,
        0.014820006,
        -0.013131463,
        0.15988831,
        0.0064275274,
        -0.0076653278,
        -0.038857676,
        0.015254312,
        -0.006424452,
        0.023108613,
        0.07357906,
        0.02665727,
        0.00575866,
        -0.0020714481,
        -0.025986703,
        0.027917072,
        -0.05469967,
        -0.021670582,
        -0.013154979,
        0.03821949,
        -0.012864586,
        0.0041407137,
        0.028950866,
        -0.0063043595,
        -0.008261838,
        0.020844104,
        0.00023263764,
        0.019758994,
        -0.019021928,
        0.03960655,
        -0.033878434,
        0.013370168,
        0.014440682,
        0.0015611759,
        -0.0060427976,
        -0.045798533,
        0.0028658975,
        0.0048241396,
        -0.026040733,
        0.02626537,
        0.019150974,
        -0.029956313,
        0.034417532,
        0.004912864,
        -0.010934778,
        0.0015013685,
        -0.022339396,
        0.020023942,
        0.005828301,
        -0.09966123,
        -0.06327092,
        0.024522135,
        -0.04826947,
        -0.020258049,
        -0.020873314,
        0.00036792032,
        -0.04074486,
        -0.019007195,
        0.0076569123,
        -0.0016037169,
        -0.014027866,
        0.0073729367,
        0.032381486,
        0.0052755023,
        0.0070434883,
        -0.012318134,
        -0.021978505,
        -0.0035620113,
        -0.035701845,
        -0.0062370175,
        -0.02363757,
        -0.03096813,
        0.00068176736,
        -0.012917327,
        0.0018843627,
        0.00052359427,
        -0.0044537387,
        -0.024308093,
        0.03562218,
        -0.011851221,
        0.028853856,
        -0.0012316285,
        0.02336089,
        0.0124050295,
        -0.03968709,
        -0.22498026,
        0.019794008,
        0.017281797,
        -0.003570257,
        0.25313136,
        -0.01618679,
        -0.014901762,
        -0.005371125,
        0.028242508,
        0.01495046,
        -0.002102732,
        -0.009359438,
        0.00038446576,
        0.038829945,
        0.03757913,
        0.061200988,
        0.039118737,
        -0.004323444,
        -0.027902763,
        0.021966223,
        0.036142662,
        0.0083741965,
        -0.014607301,
        0.013467545,
        0.015450331,
        -0.01713689,
        0.015013144,
        0.031145055,
        -0.03161453,
        -0.022872536,
        0.022965059,
        0.01465307,
        -0.040879726,
        -0.0070571224,
        0.0005238096,
        0.006517733,
        -0.05945249,
        -0.00067222246,
        -0.017303798,
        -0.02743768,
        0.051286776,
        0.010820717,
        -0.008597286,
        0.008311842,
        0.031794846,
        0.03725525,
        -0.007881769,
        0.034670442,
        -0.008120512,
        -0.0017984086,
        -0.008127016,
        -0.015096135,
        0.031332,
        0.013066103,
        -0.015996825,
        0.036567163,
        0.0023044932,
        -0.015515072,
        0.035640754,
        -0.025439778,
        0.019737234,
        -0.00048255606,
        0.027483864,
        -0.0062847566,
        0.035673726,
        0.02689843,
        -0.024476523,
        0.036291257,
        0.07619501,
        0.044448603,
        -0.02978229,
        0.0003071704,
        -0.066682085,
        -0.016464977,
        0.027141921,
        0.0015256412,
        -0.040789746,
        0.00044568328,
        -0.0073254695,
        0.020374568,
        0.009659304,
        0.021580324,
        0.00032814275,
        -0.033917915,
        -0.029009834,
        0.044985965,
        0.008687944,
        -0.040525082,
        0.01396069,
        -0.05742075,
        0.019486612,
        0.01334306,
        0.031041175,
        0.027065355,
        -0.012784972,
        0.0044180467,
        0.034939438,
        -0.013596606,
        0.020558216,
        0.011244942,
        -0.02307572,
        -0.019498749,
        -0.013778815,
        -0.0036768846,
        0.018824909,
        0.037605233,
        0.039746355,
        -0.0054461425,
        -0.01871201,
        -0.008835689,
        0.020823514,
        0.032042388,
        0.01331485,
        0.02537492,
        -0.0078030215,
        0.039240696,
        -0.021729227,
        -0.005688172,
        0.021090481,
        0.039646916,
        -0.034255978,
        -0.008763929,
        0.022813259,
        0.04913263,
        -0.008697633,
        -0.047809932,
        -0.0049542347,
        -0.000523725,
        0.00044063161,
        0.0046917875,
        0.0051231035,
        -0.04871753,
        0.010481537,
        0.001975782,
        -0.029364169,
        0.0010357029,
        0.030492049,
        -0.039915103,
        -0.008770563,
        0.027659342,
        -0.029857345,
        0.0154229775,
        0.0052343365,
        0.005864664,
        0.03145457,
        -0.041445766,
        0.014016001,
        -0.03302228,
        -0.013902694,
        -0.01625225,
        0.00993095,
        -0.01161224,
        -0.03400416,
        0.009857927,
        0.0104377465,
        0.060225435,
        -0.0093719335,
        0.0018534202,
        0.018284181,
        -0.01361248,
        0.017421937,
        -0.0038058027,
        0.042009708,
        -0.015804857,
        0.021955919,
        -0.0012992409,
        0.038149707,
        0.018156793,
        -0.062405195,
        0.013066391,
        -0.056466848,
        -0.017757474,
        -0.0028650656,
        0.0058570434,
        -0.010280581,
        0.021009846,
        0.016863098,
        -0.015731147,
        0.016432023,
        0.041244943,
        0.031222174,
        -0.0053466456,
        0.016777335,
        0.004303855,
        -0.0051430822,
        -0.01962097,
        0.00046041392,
        0.009175838,
        -0.008946787,
        -0.041479073,
        0.0012780037,
        0.01963695,
        -0.026783299,
        -0.01092655,
        0.03702143,
        0.012992049,
        0.008260065,
        -0.018874738,
        -0.01286012,
        0.016152298,
        -0.024768036,
        -0.024065694,
        0.0008564311,
        -0.003723401,
        -0.0047782045,
        0.012646516,
        0.011130584,
        0.007987915,
        -0.13179192,
        -0.018177606,
        0.02961083,
        0.010106819,
        0.008113584,
        -0.030036584,
        0.012636336,
        0.029913815,
        0.03315664,
        -0.008453596,
        -0.03339465,
        0.0021889387,
        0.013170344,
        -0.01902177,
        0.005910975,
        0.022003956,
        -0.0063015297,
        -0.0185965,
        0.0033527578,
        -0.022245914,
        -0.042567033,
        0.002801951,
        -0.17528647,
        0.0005035894,
        -0.017844167,
        -0.04551095,
        0.011306323,
        -0.030462844,
        0.0017954145,
        0.0061569316,
        0.019132044,
        0.029423045,
        0.023821782,
        0.018651243,
        0.062674895,
        0.008055076,
        0.027926216,
        0.0040267725,
        -0.0015232497,
        -0.010748787,
        -0.013262485,
        0.008980097,
        -0.033223867,
        0.0146368295,
        0.022167355,
        0.009057029,
        -0.023929827,
        -0.02951758,
        -0.0056341076,
        0.06293271,
        -0.017162772,
        0.026563834,
        0.055115834,
        0.03297112,
        0.044023864,
        0.03940343,
        0.030845787,
        -0.009692795,
        -0.00940617,
        -0.017781934,
        -0.0047045476,
        -0.017536366,
        -0.029622015,
        -0.026149537,
        0.014223205,
        0.042495977,
        -0.0290101,
        0.044529866,
        -0.0454436,
        -0.017035026,
        -0.043106273,
        0.004973654,
        0.29866093,
        -0.002671509,
        -0.035108592,
        -0.004368086,
        -0.037166778,
        -0.05845625,
        -0.0010122175,
        0.011301448,
        -0.035917412,
        -0.0042722896,
        0.0069688833,
        0.04308006,
        0.014895897,
        -0.00661524,
        -0.036040846,
        0.022869103,
        -0.004199664,
        -0.010235386,
        0.0077593494,
        -0.0121860765,
        -0.046512168,
        -0.0064643933,
        -0.0047807526,
        -0.018116102,
        0.023745356,
        -0.040249992,
        -0.031160146,
        -0.05771907,
        -0.02815563,
        -0.0068371277,
        -0.01035654,
        0.024611121,
        -0.007522822,
        0.017330028,
        0.022064786,
        0.011030672,
        -0.011998312,
        -0.0041401656,
        -0.0062133586,
        -0.04972406,
        -0.011494944,
        -0.0047495724,
        0.018067274,
        0.039112672,
        -0.019449852,
        0.0065324428,
        -0.02769223,
        -0.039807513,
        0.006461706,
        0.035815254,
        0.0017134275,
        -0.005184694,
        -0.022443162,
        -0.0072568725,
        -0.002618277,
        0.015006618,
        -0.007317327,
        0.037664324,
        -0.023994833,
        0.0054134326,
        -0.003410414,
        -0.0237863,
        0.01482158,
        -0.014767443,
        -0.015756682,
        -0.0022374734,
        0.026522176,
        0.0030798607,
        -0.012200735,
        -0.0686059,
        -0.01256213,
        0.01759631,
        0.0014242876,
        0.044622954,
        0.028350726,
        -0.008226041,
        0.015207355,
        0.0146250725,
        0.015122039,
        -0.03984472,
        -0.02007866,
        0.0028963448,
        0.039672844,
        -0.057417013,
        0.048817653,
        -0.02627826,
        0.0134779485,
        -0.008799786,
        -0.0030325444,
        -0.012617669,
        -0.00087181904,
        0.019178504,
        0.011707547,
        -0.0065853586,
        -0.008898021,
        0.015297573,
        -0.04113959,
        0.01135404,
        -0.018460345,
        0.005675249,
        -0.02876876,
        -0.0065206215,
        0.006008467,
        0.04377509,
        -0.016163269,
        -0.009146873,
        -0.0015525562,
        0.0007020318,
        -0.02461698,
        -0.0344008,
        0.012333875,
        -0.011139719,
        0.011816653,
        -0.014555361,
        -0.0003070767,
        -0.00907902,
        -0.19088055,
        0.015713643,
        0.037807066,
        -0.019069457,
        -0.008042357,
        0.049934104,
        -0.021369996,
        0.0140267825,
        0.00420878,
        0.007308135,
        -0.028600363,
        0.016940795,
        -0.05842496,
        0.006888315,
        0.065117255,
        0.020332089,
        0.014443868,
        -0.065477155,
        0.0010837859,
        -0.005974733,
        0.007969608,
        -0.07594507,
        0.0029710634,
        0.010829651,
        -0.0012731664,
        0.0017792372,
        -0.014663885,
        -0.0203348,
        0.016117094,
        -0.03351677,
        -0.031653583,
        0.0020854105,
        -0.036179002,
        0.0034623882,
        0.010883555,
        0.029086262,
        -0.037473448,
        0.02590499,
        -0.008166385,
        0.009189521,
        0.020489529,
        0.038782965,
        0.029644571,
        -0.0018531352,
        0.047954768,
        -0.014560271,
        0.03497629,
        -0.2864895,
        0.030249074,
        -0.008526756,
        -0.03771894,
        -0.03704407,
        -0.056556262,
        -0.030370766,
        -0.015169972,
        0.03480281,
        0.006294808,
        -0.0067806765,
        0.011883565,
        -0.026535155,
        0.026770437,
        -0.040663313,
        0.005396514,
        -0.0063958433,
        -0.0102125285,
        0.040829312,
        0.02465255,
        0.050618887,
        -0.02336513,
        -0.01293364,
        -0.004051999,
        0.021325089,
        -0.056525428,
        0.008540481,
        0.017834686,
        -0.022880128,
        0.005065879,
        -0.023469102,
        0.024000825,
        0.028049674,
        0.01294549,
        0.026906919,
        0.0038596892,
        -0.018538222,
        -0.01048302,
        0.068679444,
        0.020244043,
        0.018993724,
        0.019902077,
        0.017294884,
        0.010387368,
        -0.013022128,
        -0.007912021,
        0.016969386,
        -0.0005016011,
        0.014465001,
        -0.0020284972,
        0.0066795074,
        0.0014131917,
        -0.039595082,
        -0.037901003,
        -0.0056755184,
        -0.0134587595,
        -0.019023392,
        0.02653589,
        -0.010009279,
        0.012755573,
        0.021138454,
        0.024111101,
        -0.0049227146,
        -0.021821024,
        -0.0038105084,
        0.00024833335,
        0.0015837409,
        0.011216936,
        -0.0011316041,
        0.040861413,
        0.030079305,
        0.020069895,
        0.018964952,
        0.025762206,
        -0.027975056,
        0.006083612,
        0.041216183,
        -0.0198914,
        -0.037045345,
        0.009628558,
        -0.004648141,
        -0.023070302,
        -0.025827674,
        0.032872,
        0.05590265,
        -0.0074252035,
        -0.02661827,
        -0.018210603,
        0.0076413676,
        0.026913233,
        0.014321531,
        0.0049917623,
        0.029138755,
        -0.00072933227,
        0.012737821,
        -0.011692181,
        0.01370206,
        -0.019096408,
        -0.017844934,
        -0.036554847,
        0.046650723,
        -0.01349189,
        -0.02140371,
        -0.016438346,
        -0.013416906,
        0.0006781695,
        0.060341988,
        -0.020184021,
        0.006736895,
        -0.005342232,
        -0.0012715309,
        -0.023459038,
        0.021250091,
        -0.01638936,
        0.009222685,
        0.017368332,
        0.005119245,
        -0.014245158,
        0.070186354,
        -0.0136648305,
        0.015072559,
        0.011200258,
        0.0020309482,
        -0.011483067,
        0.032985736,
        0.040143743,
        -0.02111509,
        0.009001113,
        -0.016965754,
        -0.0035368428,
        -0.03147873,
        0.038750287,
        -0.025312606,
        -0.010575393,
        -0.0041843024,
        -0.025241813,
        -0.02481768,
        0.0054268795,
        -0.013957707,
        -0.005630476,
        0.016301252,
        -0.009564899,
        0.040901873,
        0.0077344235,
        -0.034312457,
        0.0070438446,
        0.06272498,
        0.04281858,
        -0.012747501,
        0.057406064,
        0.03071193,
        -0.00536834,
        -0.03017276,
        -0.019532941,
        -0.0067724953,
        -0.0092885615,
        0.042543396,
        -0.056247883,
        -0.026811935,
        0.020648511,
        -0.04053965,
        0.009476278,
        0.0073615,
        0.009893717,
        0.012109694,
        0.022592265,
        -0.060574617,
        0.010043578,
        0.016987907,
        0.03377897,
        -0.030498004,
        -0.01750739,
        0.067946605,
        0.007580749,
        0.0012963444,
        0.019019201,
        -0.0069742682,
        -0.011390442,
        0.29936674,
        0.03025758,
        -0.033098254,
        -0.020426897,
        0.019291064,
        -0.022534057,
        0.016344719,
        -0.023486739,
        0.018488541,
        -0.048147343,
        -0.010020716,
        -0.040037777,
        -0.03153086,
        -0.054456603,
        0.0065903524,
        0.024521971,
        -0.060160343,
        0.0045516137,
        0.013521374,
        -0.010743019,
        0.008624498,
        0.04089224,
        -0.021292338,
        0.002317146,
        0.02656671,
        0.024010226,
        0.020137409,
        -0.030693509,
        -0.060116753,
        0.024448955,
        0.015258921,
        -0.04637649,
        0.013759733,
        0.0059404382,
        -0.006709233,
        0.019880014,
    ];

    #[test]
    fn should_build_text_only_parts_when_no_description_and_no_image() {
        let title = Title::from("Antique Vase");
        let parts = MultimodalEmbeddingServiceImpl::build_content_parts(&title, None, None);

        assert_eq!(parts.len(), 1);
        match &parts[0] {
            ContentPart::Text { text } => assert_eq!(text, "title: Antique Vase | text: none"),
            _ => panic!("Expected text part"),
        }
    }

    #[test]
    fn should_build_text_with_description_when_description_provided() {
        let title = Title::from("Antique Vase");
        let description = Description::from("A beautiful 18th century vase");
        let parts =
            MultimodalEmbeddingServiceImpl::build_content_parts(&title, Some(&description), None);

        assert_eq!(parts.len(), 1);
        match &parts[0] {
            ContentPart::Text { text } => {
                assert_eq!(
                    text,
                    "title: Antique Vase | text: A beautiful 18th century vase"
                )
            }
            _ => panic!("Expected text part"),
        }
    }

    #[test]
    fn should_build_text_and_image_parts_when_image_data_provided() {
        let title = Title::from("Antique Vase");
        let description = Description::from("Beautiful vase");
        let image_data = Some(("image/jpeg".to_string(), "base64data".to_string()));
        let parts = MultimodalEmbeddingServiceImpl::build_content_parts(
            &title,
            Some(&description),
            image_data,
        );

        assert_eq!(parts.len(), 2);
        match &parts[0] {
            ContentPart::Text { text } => {
                assert_eq!(text, "title: Antique Vase | text: Beautiful vase")
            }
            _ => panic!("Expected text part"),
        }
        match &parts[1] {
            ContentPart::InlineData { inline_data } => {
                assert_eq!(inline_data.mime_type, "image/jpeg");
                assert_eq!(inline_data.data, "base64data");
            }
            _ => panic!("Expected inline data part"),
        }
    }

    #[test]
    fn should_build_title_only_text_when_description_is_none() {
        let title = Title::from("Rare Clock");
        let parts = MultimodalEmbeddingServiceImpl::build_content_parts(
            &title,
            None,
            Some(("image/png".to_string(), "imgdata".to_string())),
        );

        assert_eq!(parts.len(), 2);
        match &parts[0] {
            ContentPart::Text { text } => assert_eq!(text, "title: Rare Clock | text: none"),
            _ => panic!("Expected text part"),
        }
    }

    #[test]
    fn should_serialize_text_part_correctly() {
        let part = ContentPart::Text {
            text: "hello".to_string(),
        };
        let json = serde_json::to_value(&part).unwrap();
        assert_eq!(json, serde_json::json!({"text": "hello"}));
    }

    #[test]
    fn should_serialize_inline_data_part_correctly() {
        let part = ContentPart::InlineData {
            inline_data: InlineData {
                mime_type: "image/jpeg".to_string(),
                data: "abc123".to_string(),
            },
        };
        let json = serde_json::to_value(&part).unwrap();
        assert_eq!(
            json,
            serde_json::json!({"inlineData": {"mimeType": "image/jpeg", "data": "abc123"}})
        );
    }

    #[test]
    fn should_serialize_embed_content_request_correctly() {
        let request = EmbedContentRequest {
            model: "models/gemini-embedding-2",
            content: Content {
                parts: vec![ContentPart::Text {
                    text: "Test title".to_string(),
                }],
            },
            task_type: None,
        };
        let json = serde_json::to_value(&request).unwrap();
        assert_eq!(
            json,
            serde_json::json!({
                "model": "models/gemini-embedding-2",
                "content": {
                    "parts": [{"text": "Test title"}]
                }
            })
        );
    }

    #[test]
    fn should_deserialize_embed_content_response_correctly() {
        let json = r#"{"embedding": {"values": [0.1, 0.2, 0.3]}}"#;
        let response: EmbedContentResponse = serde_json::from_str(json).unwrap();
        assert_eq!(response.embedding.values, vec![0.1, 0.2, 0.3]);
    }

    #[test]
    fn should_normalize_embedding_vector_to_unit_length_when_vector_normalized() {
        let mut values = GEMINI_EXAMPLE_RESPONSE_EMBEDDING_768.to_vec();
        let original_len = values.len();

        assert_eq!(original_len, 768, "Test vector must be 768 dimensions");

        let norm = values.iter().map(|x| x * x).sum::<f32>().sqrt();
        assert!(
            norm > 0.0,
            "Expected norm to be greater than 0.0, got {}",
            norm
        );

        for v in &mut values {
            *v /= norm;
        }

        // Verify the vector still has the same dimensions (important for 768-dim vectors)
        assert_eq!(values.len(), original_len);
        assert_eq!(
            values.len(),
            768,
            "Normalized vector must still be 768 dimensions"
        );

        // Verify the L2 norm of the normalized vector is 1.0 (unit length)
        let normalized_norm = values.iter().map(|x| x * x).sum::<f32>().sqrt();
        assert!(
            (normalized_norm - 1.0).abs() < 1e-6,
            "Expected normalized 768-dim vector to have L2 norm of 1.0, got {}",
            normalized_norm
        );
    }

    // -------- Query embedding cache --------

    #[tokio::test]
    async fn should_construct_query_cache_with_configured_capacity_for_warm_lambda() {
        // The LRU cache is a private implementation detail of `MultimodalEmbeddingServiceImpl`;
        // we cannot exercise the network path in a unit test, but we can verify the cache
        // is wired with the expected capacity bound so warm Lambda invocations can hit it.
        let svc = MultimodalEmbeddingServiceImpl::new("test-key");
        let cache = svc.query_cache.lock().await;
        assert_eq!(cache.cap().get(), QUERY_EMBEDDING_CACHE_CAPACITY);
        assert_eq!(cache.len(), 0);
    }

    #[tokio::test]
    async fn should_serve_query_embedding_from_cache_when_repeated_for_paged_calls() {
        // Manually pre-populate the cache to simulate a previous successful call, then
        // verify that the cached value is returned without exercising the HTTP client.
        let svc = MultimodalEmbeddingServiceImpl::new("test-key");
        let expected = vec![0.1_f32, 0.2, 0.3];
        svc.query_cache
            .lock()
            .await
            .put("hello".to_string(), expected.clone());

        let actual = svc.embed_query("hello").await.unwrap();
        assert_eq!(expected, actual);
    }

    #[tokio::test]
    async fn should_evict_least_recently_used_entry_when_capacity_exceeded() {
        // Drive the cache directly past capacity to assert LRU semantics.
        let svc = MultimodalEmbeddingServiceImpl::new("test-key");
        {
            let mut cache = svc.query_cache.lock().await;
            for i in 0..QUERY_EMBEDDING_CACHE_CAPACITY {
                cache.put(format!("q-{i}"), vec![i as f32]);
            }
            // Touch the oldest key to promote it to MRU, then insert a new entry which
            // should evict the *next* oldest (q-1) instead.
            assert!(cache.get("q-0").is_some());
            cache.put("q-new".to_string(), vec![-1.0]);
        }
        let cache = svc.query_cache.lock().await;
        assert_eq!(cache.len(), QUERY_EMBEDDING_CACHE_CAPACITY);
        assert!(cache.peek("q-0").is_some(), "promoted entry must survive");
        assert!(
            cache.peek("q-1").is_none(),
            "least-recently-used must be evicted"
        );
        assert!(cache.peek("q-new").is_some());
    }
}

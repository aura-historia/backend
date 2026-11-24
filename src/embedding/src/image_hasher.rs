//! The perceptual hash of the image a product embedding would send, to tell whether a listing's
//! new first image shows the picture its embedding was computed with.

use crate::{EmbeddingImageUrl, ImageHash, product_image::fetch_embedding_image_hash};
use image_fetcher::ImageFetcher;

#[async_trait::async_trait]
pub trait EmbeddingImageHasher: Send + Sync {
    /// The hash of the image at `url` as an embedding would send it, computed like the hash a
    /// batch reports in [`EmbeddedImage`](crate::EmbeddedImage). `None` when the image cannot be
    /// fetched or decoded; the cause is logged.
    async fn image_hash(&self, url: &EmbeddingImageUrl) -> Option<ImageHash>;
}

/// Fetches the image with the bounded [`ImageFetcher`] the embedding uses.
pub struct FetchingEmbeddingImageHasher {
    fetcher: ImageFetcher,
}

impl FetchingEmbeddingImageHasher {
    pub fn new() -> Self {
        Self {
            fetcher: ImageFetcher::new(),
        }
    }
}

impl Default for FetchingEmbeddingImageHasher {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl EmbeddingImageHasher for FetchingEmbeddingImageHasher {
    async fn image_hash(&self, url: &EmbeddingImageUrl) -> Option<ImageHash> {
        fetch_embedding_image_hash(&self.fetcher, url).await
    }
}

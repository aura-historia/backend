use crate::shop_document::ShopDocument;

#[async_trait::async_trait]
#[mockall::automock]
pub trait ShopOpenSearchRepository {
    async fn create_shop_document(&self, document: ShopDocument) -> Result<(), opensearch::Error>;
}

#[derive(Debug, Clone)]
pub struct ShopOpenSearchRepositoryImpl<'a> {
    client: &'a opensearch::OpenSearch,
}

impl<'a> ShopOpenSearchRepositoryImpl<'a> {
    pub fn new(client: &'a opensearch::OpenSearch) -> Self {
        Self { client }
    }
}

#[async_trait::async_trait]
impl<'a> ShopOpenSearchRepository for ShopOpenSearchRepositoryImpl<'a> {
    async fn create_shop_document(&self, document: ShopDocument) -> Result<(), opensearch::Error> {
        todo!()
    }
}

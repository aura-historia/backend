use common::{opensearch::search_response::SearchResponse, page::Page, sort::Sort};
use shop_core::sort_shop_field::SortShopField;

use crate::{shop_document::ShopDocument, shop_search::ShopSearch};

#[async_trait::async_trait]
#[mockall::automock]
pub trait ShopOpenSearchRepository {
    async fn create_shop_document(&self, document: ShopDocument) -> Result<(), opensearch::Error>;

    async fn search_shop_documents(
        &self,
        search: &ShopSearch,
        sort: &Option<Sort<SortShopField>>,
        page: &Option<Page>,
    ) -> Result<SearchResponse<ShopDocument>, opensearch::Error>;
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

    async fn search_shop_documents(
        &self,
        search: &ShopSearch,
        sort: &Option<Sort<SortShopField>>,
        page: &Option<Page>,
    ) -> Result<SearchResponse<ShopDocument>, opensearch::Error> {
        todo!()
    }
}

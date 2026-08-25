use crate::use_cases::{
    CreateProductListingUseCase, DeleteProductListingUseCase, GetProductListingEventsUseCase,
    GetProductListingUseCase, GetSimilarProductListingsUseCase, SearchProductListingsUseCase,
    UpdateProductListingUseCase, UpsertProductListingUseCase,
};
use std::sync::Arc;

pub struct ProductListingUseCases {
    pub create: Arc<dyn CreateProductListingUseCase>,
    pub update: Arc<dyn UpdateProductListingUseCase>,
    pub upsert: Arc<dyn UpsertProductListingUseCase>,
    pub delete: Arc<dyn DeleteProductListingUseCase>,
    pub get: Arc<dyn GetProductListingUseCase>,
    pub get_history: Arc<dyn GetProductListingEventsUseCase>,
    pub get_similar: Arc<dyn GetSimilarProductListingsUseCase>,
    pub search: Arc<dyn SearchProductListingsUseCase>,
}

pub struct ProductListingUseCasesInput {
    pub create: Arc<dyn CreateProductListingUseCase>,
    pub update: Arc<dyn UpdateProductListingUseCase>,
    pub upsert: Arc<dyn UpsertProductListingUseCase>,
    pub delete: Arc<dyn DeleteProductListingUseCase>,
    pub get: Arc<dyn GetProductListingUseCase>,
    pub get_history: Arc<dyn GetProductListingEventsUseCase>,
    pub get_similar: Arc<dyn GetSimilarProductListingsUseCase>,
    pub search: Arc<dyn SearchProductListingsUseCase>,
}

impl ProductListingUseCases {
    pub fn new(input: ProductListingUseCasesInput) -> Self {
        Self {
            create: input.create,
            update: input.update,
            upsert: input.upsert,
            delete: input.delete,
            get: input.get,
            get_history: input.get_history,
            get_similar: input.get_similar,
            search: input.search,
        }
    }
}

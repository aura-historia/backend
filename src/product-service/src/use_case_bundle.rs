use crate::use_cases::{
    CreateProductUseCase, DeleteProductUseCase, GetProductEventsUseCase, GetProductUseCase,
    GetSimilarProductsUseCase, SearchProductsUseCase, UpdateProductUseCase, UpsertProductUseCase,
};
use std::sync::Arc;

pub struct ProductUseCases {
    pub create: Arc<dyn CreateProductUseCase>,
    pub update: Arc<dyn UpdateProductUseCase>,
    pub upsert: Arc<dyn UpsertProductUseCase>,
    pub delete: Arc<dyn DeleteProductUseCase>,
    pub get: Arc<dyn GetProductUseCase>,
    pub get_history: Arc<dyn GetProductEventsUseCase>,
    pub get_similar: Arc<dyn GetSimilarProductsUseCase>,
    pub search: Arc<dyn SearchProductsUseCase>,
}

pub struct ProductUseCasesInput {
    pub create: Arc<dyn CreateProductUseCase>,
    pub update: Arc<dyn UpdateProductUseCase>,
    pub upsert: Arc<dyn UpsertProductUseCase>,
    pub delete: Arc<dyn DeleteProductUseCase>,
    pub get: Arc<dyn GetProductUseCase>,
    pub get_history: Arc<dyn GetProductEventsUseCase>,
    pub get_similar: Arc<dyn GetSimilarProductsUseCase>,
    pub search: Arc<dyn SearchProductsUseCase>,
}

impl ProductUseCases {
    pub fn new(input: ProductUseCasesInput) -> Self {
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

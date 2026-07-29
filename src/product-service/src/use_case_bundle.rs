use crate::use_cases::{
    CreateProductUseCase, DeleteProductUseCase, GetProductUseCase, SearchProductsUseCase,
    UpdateProductUseCase,
};
use std::sync::Arc;

pub struct ProductUseCases {
    pub create: Arc<dyn CreateProductUseCase>,
    pub update: Arc<dyn UpdateProductUseCase>,
    pub delete: Arc<dyn DeleteProductUseCase>,
    pub get: Arc<dyn GetProductUseCase>,
    pub search: Arc<dyn SearchProductsUseCase>,
}

pub struct ProductUseCasesInput {
    pub create: Arc<dyn CreateProductUseCase>,
    pub update: Arc<dyn UpdateProductUseCase>,
    pub delete: Arc<dyn DeleteProductUseCase>,
    pub get: Arc<dyn GetProductUseCase>,
    pub search: Arc<dyn SearchProductsUseCase>,
}

impl ProductUseCases {
    pub fn new(input: ProductUseCasesInput) -> Self {
        Self {
            create: input.create,
            update: input.update,
            delete: input.delete,
            get: input.get,
            search: input.search,
        }
    }
}

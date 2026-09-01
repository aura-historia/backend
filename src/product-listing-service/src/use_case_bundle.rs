use crate::use_cases::{
    CreateProductListingUseCase, GetProductListingHistoryUseCase, GetProductListingUseCase,
    GetSimilarProductListingsUseCase, SearchProductListingsUseCase, UpdateProductListingUseCase,
    UpsertProductListingUseCase, WithdrawProductListingUseCase,
};
use std::sync::Arc;

pub struct ProductListingUseCases {
    pub create: Arc<dyn CreateProductListingUseCase>,
    pub update: Arc<dyn UpdateProductListingUseCase>,
    pub upsert: Arc<dyn UpsertProductListingUseCase>,
    pub withdraw: Arc<dyn WithdrawProductListingUseCase>,
    pub get: Arc<dyn GetProductListingUseCase>,
    pub get_history: Arc<dyn GetProductListingHistoryUseCase>,
    pub get_similar: Arc<dyn GetSimilarProductListingsUseCase>,
    pub search: Arc<dyn SearchProductListingsUseCase>,
}

pub struct ProductListingUseCasesInput {
    pub create: Arc<dyn CreateProductListingUseCase>,
    pub update: Arc<dyn UpdateProductListingUseCase>,
    pub upsert: Arc<dyn UpsertProductListingUseCase>,
    pub withdraw: Arc<dyn WithdrawProductListingUseCase>,
    pub get: Arc<dyn GetProductListingUseCase>,
    pub get_history: Arc<dyn GetProductListingHistoryUseCase>,
    pub get_similar: Arc<dyn GetSimilarProductListingsUseCase>,
    pub search: Arc<dyn SearchProductListingsUseCase>,
}

impl ProductListingUseCases {
    pub fn new(input: ProductListingUseCasesInput) -> Self {
        Self {
            create: input.create,
            update: input.update,
            upsert: input.upsert,
            withdraw: input.withdraw,
            get: input.get,
            get_history: input.get_history,
            get_similar: input.get_similar,
            search: input.search,
        }
    }
}

pub mod commands;
pub mod queries;

pub use commands::create_product::{
    CreateProductCommand, CreateProductError, CreateProductResult, CreateProductUseCase,
};
pub use commands::delete_product::{
    DeleteProductCommand, DeleteProductError, DeleteProductResult, DeleteProductUseCase,
};
pub use commands::update_product::{
    UpdateProductCommand, UpdateProductError, UpdateProductResult, UpdateProductUseCase,
};
pub use queries::get_product::{
    GetProductError, GetProductRequest, GetProductUseCase, ProductDetailsView,
};
pub use queries::search_products::{
    ProductSummary, SearchProductsError, SearchProductsRequest, SearchProductsResult,
    SearchProductsUseCase,
};

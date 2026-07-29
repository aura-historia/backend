pub mod commands;
pub mod queries;

pub use commands::create_product::{
    CreateProductCommand, CreateProductError, CreateProductHandler, CreateProductResult,
    CreateProductUseCase,
};
pub use commands::delete_product::{
    DeleteProductCommand, DeleteProductError, DeleteProductHandler, DeleteProductResult,
    DeleteProductUseCase,
};
pub use commands::update_product::{
    UpdateProductCommand, UpdateProductError, UpdateProductHandler, UpdateProductResult,
    UpdateProductUseCase,
};
pub use queries::get_product::{
    GetProductError, GetProductHandler, GetProductRequest, GetProductUseCase, ProductDetailsView,
};
pub use queries::search_products::{
    ProductSummary, SearchProductsError, SearchProductsHandler, SearchProductsRequest,
    SearchProductsResult, SearchProductsUseCase,
};

#![allow(dead_code)]

pub mod create_product;
pub mod delete_product;
pub mod update_product;

pub use create_product::PostgresCreateProductHandler;
pub use delete_product::PostgresDeleteProductHandler;
pub use update_product::PostgresUpdateProductHandler;

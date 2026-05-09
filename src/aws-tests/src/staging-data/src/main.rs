#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    staging_tests::reset().await;
    Ok(())
}

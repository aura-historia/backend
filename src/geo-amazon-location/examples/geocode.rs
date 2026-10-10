//! Opt-in manual composition root. Reads a public dealer query from stdin; no API key,
//! resource provisioning, result persistence, or sensitive diagnostic output.
use aws_sdk_geoplaces::config::Region;
use geo::AddressText;
use geo_amazon_location::{AmazonLocationConfig, AmazonLocationGeocoder, REGION};
use geo_service::geocoding::{
    Geocode, GeocodeHandler, GeocodingOutcome, GeocodingPurpose, GeocodingRequest,
};
use std::io::{self, Read};

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), application::error::BoxError> {
    let mut query = String::new();
    io::stdin()
        .take(AddressText::MAX_BYTES as u64 + 1)
        .read_to_string(&mut query)?;
    let request =
        GeocodingRequest::new(AddressText::new(query)?, GeocodingPurpose::DealerPreview, 5)?;
    let sdk = aws_config::defaults(aws_config::BehaviorVersion::latest())
        .region(Region::new(REGION))
        .load()
        .await;
    let provider = AmazonLocationGeocoder::new(
        aws_sdk_geoplaces::Client::new(&sdk),
        AmazonLocationConfig::default(),
    )?;
    match GeocodeHandler::new(provider).geocode(&request).await {
        Ok(result) => match result.outcome {
            GeocodingOutcome::NoMatch => println!("No match"),
            GeocodingOutcome::Candidates(candidates) => {
                println!("Candidates: {}", candidates.as_slice().len());
                for candidate in candidates.as_slice() {
                    println!(
                        "{:?}; issues: {:?}",
                        candidate.usability(),
                        candidate.issues()
                    );
                }
            }
        },
        Err(error) => {
            // Reporting the SDK source chain can expose the address or provider payload.
            eprintln!("{error}");
            std::process::exit(1);
        }
    }
    Ok(())
}

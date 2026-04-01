use fake::rand::RngExt;
use fake::rand::seq::IndexedRandom;
use fake::{Dummy, Fake, Faker};
use url::Url;

#[derive(Debug, Clone, PartialEq)]
pub struct ImageUrl(Url);

impl From<ImageUrl> for Url {
    fn from(safe_url: ImageUrl) -> Self {
        safe_url.0
    }
}

const IMAGE_RATIOS: [&str; 16] = [
    "1920/1080",
    "1280/720",
    "768/1024",
    "1080/1350",
    "1200/1500",
    "1600/2000",
    "1080/1920",
    "720/1280",
    "1440/2560",
    "900/600",
    "800/600",
    "1000/800",
    "512/512",
    "360/640",
    "640/360",
    "900/300",
];

impl Dummy<Faker> for ImageUrl {
    fn dummy_with_rng<R: RngExt + ?Sized>(_: &Faker, rng: &mut R) -> Self {
        let seed = Faker.fake::<String>();
        let ratio: &str = IMAGE_RATIOS.choose(rng).unwrap();
        let image_url = Url::parse(&format!("https://picsum.photos/seed/{seed}/{ratio}")).unwrap();
        ImageUrl(image_url)
    }
}

#[cfg(test)]
mod tests {
    use crate::fake::url::ImageUrl;
    use fake::{Fake, Faker};

    #[test]
    fn should_fake_image_url() {
        let _ = Faker.fake::<ImageUrl>();
    }
}

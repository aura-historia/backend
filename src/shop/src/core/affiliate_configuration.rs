use url::Url;

#[derive(Debug, Clone, PartialEq)]
pub enum AffiliateConfiguration {
    Catawiki,
}

impl AffiliateConfiguration {
    pub fn build_url(&self, deeplink: &Url) -> Url {
        match self {
            AffiliateConfiguration::Catawiki => {
                let encoded = percent_encode_for_path(deeplink.as_str());
                Url::parse(&format!(
                    "https://prf.hn/click/camref:1110lF73C/pubref:aurahistoria/destination:{encoded}"
                ))
                .expect("Catawiki affiliate URL is always valid")
            }
        }
    }
}

fn percent_encode_for_path(input: &str) -> String {
    let mut result = String::with_capacity(input.len() * 3);
    for byte in input.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                result.push(byte as char);
            }
            _ => {
                result.push('%');
                result.push(
                    char::from_digit((byte >> 4) as u32, 16)
                        .unwrap()
                        .to_ascii_uppercase(),
                );
                result.push(
                    char::from_digit((byte & 0xf) as u32, 16)
                        .unwrap()
                        .to_ascii_uppercase(),
                );
            }
        }
    }
    result
}

#[cfg(feature = "test-data")]
mod faker {
    use super::*;
    use fake::{Dummy, Faker};

    impl Dummy<Faker> for AffiliateConfiguration {
        fn dummy_with_rng<R: fake::RngExt + ?Sized>(_config: &Faker, _rng: &mut R) -> Self {
            AffiliateConfiguration::Catawiki
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_build_catawiki_affiliate_url_when_given_deeplink() {
        let deeplink = Url::parse("https://www.catawiki.com/l/12345").unwrap();
        let config = AffiliateConfiguration::Catawiki;

        let result = config.build_url(&deeplink);

        assert_eq!(
            result.as_str(),
            "https://prf.hn/click/camref:1110lF73C/pubref:aurahistoria/destination:https%3A%2F%2Fwww.catawiki.com%2Fl%2F12345"
        );
    }

    #[test]
    fn should_encode_query_params_in_catawiki_affiliate_url() {
        let deeplink =
            Url::parse("https://www.catawiki.com/l/12345?ref=test&utm_source=x").unwrap();
        let config = AffiliateConfiguration::Catawiki;

        let result = config.build_url(&deeplink);

        assert!(result.as_str().contains("destination:https%3A%2F%2F"));
        assert!(result.as_str().contains("%3F")); // '?' encoded
        assert!(result.as_str().contains("%3D")); // '=' encoded
        assert!(result.as_str().contains("%26")); // '&' encoded
    }
}

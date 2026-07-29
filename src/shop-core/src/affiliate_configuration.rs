use percent_encoding::{AsciiSet, CONTROLS, utf8_percent_encode};
use url::Url;

/// Encodes characters that must be percent-encoded inside the Partnerize
/// `destination:` path segment while keeping unreserved characters
/// (letters, digits, `-`, `.`, `_`, `~`) intact.
const PARTNERIZE_DESTINATION: &AsciiSet = &CONTROLS
    .add(b' ')
    .add(b'"')
    .add(b'<')
    .add(b'>')
    .add(b'`')
    .add(b'#')
    .add(b'%')
    .add(b'?')
    .add(b'{')
    .add(b'}')
    .add(b'/')
    .add(b':')
    .add(b'@')
    .add(b'!')
    .add(b'$')
    .add(b'&')
    .add(b'\'')
    .add(b'(')
    .add(b')')
    .add(b'*')
    .add(b'+')
    .add(b',')
    .add(b';')
    .add(b'=')
    .add(b'[')
    .add(b']');

#[derive(Debug, Clone, PartialEq)]
pub enum AffiliateConfiguration {
    Partnerize { camref: String },
}

impl AffiliateConfiguration {
    pub fn build_url(&self, deeplink: &Url) -> Url {
        match self {
            AffiliateConfiguration::Partnerize { camref } => {
                let encoded =
                    utf8_percent_encode(deeplink.as_str(), PARTNERIZE_DESTINATION).to_string();
                Url::parse(&format!(
                    "https://prf.hn/click/camref:{camref}/pubref:aurahistoria/destination:{encoded}"
                ))
                .expect("Partnerize affiliate URL is always valid")
            }
        }
    }
}

#[cfg(feature = "test-data")]
mod faker {
    use super::*;
    use fake::{Dummy, Faker};

    impl Dummy<Faker> for AffiliateConfiguration {
        fn dummy_with_rng<R: fake::RngExt + ?Sized>(_config: &Faker, rng: &mut R) -> Self {
            use fake::Fake;
            AffiliateConfiguration::Partnerize {
                camref: (10..20).fake_with_rng::<String, _>(rng),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_build_partnerize_affiliate_url_when_given_deeplink() {
        let deeplink = Url::parse("https://www.catawiki.com/l/12345").unwrap();
        let config = AffiliateConfiguration::Partnerize {
            camref: "1110lF73C".to_string(),
        };

        let result = config.build_url(&deeplink);

        assert_eq!(
            result.as_str(),
            "https://prf.hn/click/camref:1110lF73C/pubref:aurahistoria/destination:https%3A%2F%2Fwww.catawiki.com%2Fl%2F12345"
        );
    }

    #[test]
    fn should_encode_query_params_in_partnerize_affiliate_url() {
        let deeplink =
            Url::parse("https://www.catawiki.com/l/12345?ref=test&utm_source=x").unwrap();
        let config = AffiliateConfiguration::Partnerize {
            camref: "1110lF73C".to_string(),
        };

        let result = config.build_url(&deeplink);

        assert!(result.as_str().contains("destination:https%3A%2F%2F"));
        assert!(result.as_str().contains("%3F")); // '?' encoded
        assert!(result.as_str().contains("%3D")); // '=' encoded
        assert!(result.as_str().contains("%26")); // '&' encoded
    }

    #[test]
    fn should_keep_unreserved_destination_chars_when_building_partnerize_affiliate_url() {
        let deeplink = Url::parse("https://example.com/a-Z_0.9~").unwrap();
        let config = AffiliateConfiguration::Partnerize {
            camref: "1110lF73C".to_string(),
        };

        let result = config.build_url(&deeplink);

        assert!(result.as_str().ends_with("%2Fa-Z_0.9~"));
    }
}

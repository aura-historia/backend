//! Recognising a bot challenge or interstitial a shop answers instead of the requested page.
//!
//! A challenge is a fetch outcome, not a page: it is never extracted, captured or used to judge a
//! schema. Recognition is conservative so an ordinary page is never taken for one. It needs an
//! answer a challenge is served with (a success status or 403) and one well-known marker: a
//! vendor's challenge header, a vendor's challenge-page script or redirect, or the exact title of
//! a known interstitial on a page small enough to be one. A vendor's name, cookie or ordinary
//! script on a page is no evidence, because many healthy shops sit behind the same vendors. The
//! contract is in `docs/crawler/networking.md` (bot challenges).

use reqwest::header::HeaderMap;

/// Largest body read for body markers. Challenge pages are small; a page above this size is never
/// taken for one on its body alone, which also bounds the scan.
const MAX_CHALLENGE_BODY_BYTES: usize = 128 * 1024;

/// Who served a recognised challenge, when the evidence names a vendor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChallengeVendor {
    Cloudflare,
    AwsWaf,
    DataDome,
    SiteGround,
    /// A known interstitial whose markup names no vendor.
    Unattributed,
}

/// A recognised challenge and the marker that proved it, for logs and failure details.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BotChallenge {
    pub vendor: ChallengeVendor,
    pub evidence: &'static str,
}

/// Header name, the value that marks a challenge (ASCII case-insensitive), vendor and evidence.
const CHALLENGE_HEADERS: &[(&str, &[&str], ChallengeVendor, &str)] = &[
    (
        "cf-mitigated",
        &["challenge"],
        ChallengeVendor::Cloudflare,
        "cf_mitigated_header",
    ),
    (
        "x-amzn-waf-action",
        &["challenge", "captcha"],
        ChallengeVendor::AwsWaf,
        "aws_waf_action_header",
    ),
    (
        "sg-captcha",
        &["challenge"],
        ChallengeVendor::SiteGround,
        "sg_captcha_header",
    ),
];

/// Lowercase markup only a vendor's challenge page carries, never a page it serves through.
const CHALLENGE_BODY_MARKERS: &[(&str, ChallengeVendor, &str)] = &[
    (
        "window._cf_chl_opt",
        ChallengeVendor::Cloudflare,
        "cloudflare_challenge_script",
    ),
    (
        "captcha-delivery.com",
        ChallengeVendor::DataDome,
        "datadome_captcha_script",
    ),
    (
        "/.well-known/sgcaptcha/",
        ChallengeVendor::SiteGround,
        "sg_captcha_redirect",
    ),
];

/// Exact titles of known interstitials, lowercase with `…` written as `...` and whitespace
/// collapsed.
const CHALLENGE_TITLES: &[(&str, ChallengeVendor)] = &[
    ("just a moment...", ChallengeVendor::Cloudflare),
    (
        "attention required! | cloudflare",
        ChallengeVendor::Cloudflare,
    ),
    ("one moment, please...", ChallengeVendor::Unattributed),
    ("einen moment bitte...", ChallengeVendor::Unattributed),
];

/// Whether a response with `status` can be a challenge: shops serve them as a success or as 403.
/// Other statuses keep their own meaning, such as 429 and 503 asking for less traffic.
pub fn may_be_challenge_status(status: u16) -> bool {
    (200..300).contains(&status) || status == 403
}

/// The challenge a response with `status`, `headers` and `body` is, or `None` for every response
/// that is not provably one.
pub fn detect_bot_challenge(status: u16, headers: &HeaderMap, body: &[u8]) -> Option<BotChallenge> {
    if !may_be_challenge_status(status) {
        return None;
    }
    challenge_from_headers(headers).or_else(|| challenge_from_body(body))
}

fn challenge_from_headers(headers: &HeaderMap) -> Option<BotChallenge> {
    CHALLENGE_HEADERS
        .iter()
        .find(|(name, values, _, _)| {
            headers.get_all(*name).iter().any(|value| {
                value.to_str().is_ok_and(|value| {
                    values
                        .iter()
                        .any(|marker| value.trim().eq_ignore_ascii_case(marker))
                })
            })
        })
        .map(|(_, _, vendor, evidence)| BotChallenge {
            vendor: *vendor,
            evidence,
        })
}

fn challenge_from_body(body: &[u8]) -> Option<BotChallenge> {
    if body.len() > MAX_CHALLENGE_BODY_BYTES {
        return None;
    }
    let text = String::from_utf8_lossy(body).to_lowercase();
    if let Some((_, vendor, evidence)) = CHALLENGE_BODY_MARKERS
        .iter()
        .find(|(marker, _, _)| text.contains(marker))
    {
        return Some(BotChallenge {
            vendor: *vendor,
            evidence,
        });
    }
    let title = normalized_title(&text)?;
    CHALLENGE_TITLES
        .iter()
        .find(|(known, _)| *known == title)
        .map(|(_, vendor)| BotChallenge {
            vendor: *vendor,
            evidence: "interstitial_title",
        })
}

/// The text of the first `<title>` of lowercase `html`, with entities and whitespace normalised
/// as `CHALLENGE_TITLES` writes them.
fn normalized_title(html: &str) -> Option<String> {
    let open = html.find("<title")?;
    let content_start = open + html[open..].find('>')? + 1;
    let content_len = html[content_start..].find("</title")?;
    let title = html[content_start..content_start + content_len]
        .replace("&hellip;", "...")
        .replace("&#8230;", "...")
        .replace('…', "...")
        .replace("&amp;", "&")
        .replace("&nbsp;", " ");
    Some(title.split_whitespace().collect::<Vec<_>>().join(" "))
}

#[cfg(test)]
mod tests {
    use super::*;
    use reqwest::header::HeaderValue;

    const CLOUDFLARE_CHALLENGE: &str = r#"<!DOCTYPE html><html lang="en-US"><head>
<title>Just a moment...</title><meta http-equiv="refresh" content="390"></head>
<body><noscript>Enable JavaScript and cookies to continue</noscript>
<script>(function(){window._cf_chl_opt={cvId: '3',cZone: "shop.example"};
var a = document.createElement('script');a.src = '/cdn-cgi/challenge-platform/h/b/orchestrate/chl_page/v1';
document.getElementsByTagName('head')[0].appendChild(a);}());</script></body></html>"#;

    /// The German bot-check page a WordPress shop served the crawler between ordinary pages.
    const GERMAN_BOT_CHECK: &str = "<html><head><meta charset=\"utf-8\">\
<title>Einen Moment bitte\u{2026}</title></head><body><h2>Ihre Anfrage wird gepr\u{fc}ft</h2>\
<script>setTimeout(function(){location.reload()},3000)</script></body></html>";

    const SITEGROUND_CAPTCHA: &str = r#"<html><head><meta http-equiv="refresh" content="0;/.well-known/sgcaptcha/?r=%2Fproduct%2Fchair%2F"></head></html>"#;

    /// An ordinary product page of a shop behind Cloudflare: the vendor's injected detection
    /// script, beacon and challenge-host policy mean nothing about the page.
    const CLOUDFLARE_FRONTED_PRODUCT: &str = r#"<html><head><title>Georgian Mahogany Chest c.1820 - Antiques</title>
<script src="/cdn-cgi/challenge-platform/scripts/jsd/main.js"></script>
<script defer src="https://static.cloudflareinsights.com/beacon.min.js"></script></head>
<body><h1>Georgian Mahogany Chest c.1820</h1><p>Just a moment... of your time to admire this chest.</p>
<span class="price">£2,400</span></body></html>"#;

    fn headers(pairs: &[(&'static str, &'static str)]) -> HeaderMap {
        let mut headers = HeaderMap::new();
        for (name, value) in pairs {
            headers.append(*name, HeaderValue::from_static(value));
        }
        headers
    }

    fn detect(
        status: u16,
        header_pairs: &[(&'static str, &'static str)],
        body: &str,
    ) -> Option<BotChallenge> {
        detect_bot_challenge(status, &headers(header_pairs), body.as_bytes())
    }

    mod recognises {
        use super::*;

        #[test]
        fn should_recognise_a_cloudflare_challenge_by_its_header() {
            let challenge = detect(403, &[("cf-mitigated", "challenge")], "").unwrap();
            assert_eq!(challenge.vendor, ChallengeVendor::Cloudflare);
            assert_eq!(challenge.evidence, "cf_mitigated_header");
        }

        #[test]
        fn should_recognise_a_cloudflare_challenge_page_by_its_script() {
            let challenge = detect(403, &[("server", "cloudflare")], CLOUDFLARE_CHALLENGE).unwrap();
            assert_eq!(challenge.vendor, ChallengeVendor::Cloudflare);
            assert_eq!(challenge.evidence, "cloudflare_challenge_script");
        }

        #[test]
        fn should_recognise_an_aws_waf_challenge_answered_with_202() {
            let challenge = detect(202, &[("x-amzn-waf-action", "challenge")], "").unwrap();
            assert_eq!(challenge.vendor, ChallengeVendor::AwsWaf);
        }

        #[test]
        fn should_recognise_a_siteground_interstitial_answered_with_202() {
            let by_header = detect(202, &[("sg-captcha", "challenge")], "").unwrap();
            let by_body = detect(202, &[], SITEGROUND_CAPTCHA).unwrap();
            assert_eq!(by_header.vendor, ChallengeVendor::SiteGround);
            assert_eq!(by_body.evidence, "sg_captcha_redirect");
        }

        #[test]
        fn should_recognise_a_datadome_captcha_page() {
            let page = r#"<html><head><title>shop.example</title></head><body>
<script>var dd={'rt':'c','cid':'AHrl','hsh':'2211F5','host':'geo.captcha-delivery.com'}</script>
<script src="https://ct.captcha-delivery.com/c.js"></script></body></html>"#;
            let challenge = detect(403, &[("x-datadome", "protected")], page).unwrap();
            assert_eq!(challenge.vendor, ChallengeVendor::DataDome);
        }

        #[test]
        fn should_recognise_a_bot_check_page_served_with_200_by_its_exact_title() {
            let challenge = detect(200, &[], GERMAN_BOT_CHECK).unwrap();
            assert_eq!(challenge.vendor, ChallengeVendor::Unattributed);
            assert_eq!(challenge.evidence, "interstitial_title");
        }

        #[test]
        fn should_read_the_title_with_entities_and_spacing() {
            let page =
                "<html><head><TITLE>\n  One moment,&nbsp;please&hellip; </TITLE></head></html>";
            assert!(detect(200, &[], page).is_some());
        }
    }

    mod leaves_alone {
        use super::*;

        #[test]
        fn should_not_take_a_cloudflare_fronted_product_page_for_a_challenge() {
            let page_headers = [
                ("server", "cloudflare"),
                ("cf-ray", "8f0a1b2c3d4e5f60-FRA"),
                ("set-cookie", "__cf_bm=abc; path=/"),
                (
                    "permissions-policy",
                    r#"private-state-token-issuance=(self "https://challenges.cloudflare.com")"#,
                ),
            ];
            assert_eq!(detect(200, &page_headers, CLOUDFLARE_FRONTED_PRODUCT), None);
        }

        #[test]
        fn should_not_take_a_datadome_protected_page_for_a_challenge() {
            let page = r#"<html><head><title>Oak Chair</title>
<script src="https://js.datadome.co/tags.js" async></script></head><body><h1>Oak Chair</h1></body></html>"#;
            assert_eq!(detect(200, &[("x-datadome", "protected")], page), None);
        }

        #[test]
        fn should_not_take_a_page_whose_title_only_contains_a_challenge_title() {
            let page =
                "<html><head><title>Just a moment... - Victorian Clock</title></head></html>";
            assert_eq!(detect(200, &[], page), None);
        }

        #[test]
        fn should_not_read_challenge_markers_on_other_statuses() {
            assert_eq!(
                detect(503, &[("cf-mitigated", "challenge")], CLOUDFLARE_CHALLENGE),
                None
            );
            assert_eq!(detect(429, &[], GERMAN_BOT_CHECK), None);
            assert_eq!(detect(404, &[], GERMAN_BOT_CHECK), None);
        }

        #[test]
        fn should_not_take_a_header_without_a_challenge_value_for_a_challenge() {
            assert_eq!(detect(403, &[("cf-mitigated", "block")], ""), None);
            assert_eq!(detect(200, &[("x-amzn-waf-action", "allow")], ""), None);
        }

        #[test]
        fn should_not_read_body_markers_on_a_page_larger_than_a_challenge() {
            let large = format!(
                "{GERMAN_BOT_CHECK}{}",
                "<p>catalogue</p>".repeat(MAX_CHALLENGE_BODY_BYTES / 16 + 1)
            );
            assert_eq!(detect(200, &[], &large), None);
        }
    }
}

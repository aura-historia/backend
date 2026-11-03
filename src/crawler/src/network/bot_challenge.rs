//! Recognising a bot challenge or interstitial a shop answers instead of the requested page.
//!
//! A challenge is a fetch outcome, not a page: it is never extracted, captured or used to judge a
//! schema. Recognition is conservative so an ordinary page is never taken for one. It needs an
//! answer a challenge is served with (a success status or 403, and 405 for AWS WAF's captcha
//! only) and one well-known marker: a vendor's challenge header, a vendor's challenge-page script
//! or redirect, or the exact title of a known interstitial on a page small enough to be one. A
//! vendor's name, cookie or ordinary script on a page is no evidence, because many healthy shops
//! sit behind the same vendors.
//!
//! A firewall's block page is no challenge: no client passes it, so it is recognised apart, as a
//! [`HardBlock`], and keeps the meaning of the refusal it was answered with. The contract is in
//! `docs/crawler/networking.md` (bot challenges).

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

/// A firewall's page refusing the client outright, which no challenge passes, and the marker that
/// proved it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HardBlock {
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
    // The script of DataDome's captcha or device-check page itself; a protected page's policy
    // or preconnect hints name the same host without it.
    (
        "ct.captcha-delivery.com/c.js",
        ChallengeVendor::DataDome,
        "datadome_captcha_script",
    ),
    (
        "ct.captcha-delivery.com/i.js",
        ChallengeVendor::DataDome,
        "datadome_captcha_script",
    ),
    (
        "/.well-known/sgcaptcha/",
        ChallengeVendor::SiteGround,
        "sg_captcha_redirect",
    ),
    // The key material AWS WAF's challenge and captcha pages hand their own script; a page AWS
    // WAF lets through carries at most its integration SDK.
    (
        "window.gokuprops",
        ChallengeVendor::AwsWaf,
        "aws_waf_challenge_page",
    ),
];

/// Exact titles of known interstitials, lowercase with `…` written as `...` and whitespace
/// collapsed.
const CHALLENGE_TITLES: &[(&str, ChallengeVendor)] = &[
    ("just a moment...", ChallengeVendor::Cloudflare),
    ("one moment, please...", ChallengeVendor::Unattributed),
    ("einen moment bitte...", ChallengeVendor::Unattributed),
];

/// The status AWS WAF answers its captcha with. Only AWS WAF's own evidence makes such an answer a
/// challenge; any other 405 keeps its meaning.
const AWS_WAF_CAPTCHA_STATUS: u16 = 405;

/// The status a firewall's block page is served with.
const HARD_BLOCK_STATUS: u16 = 403;

/// The exact title of Cloudflare's block page, normalised as `CHALLENGE_TITLES` writes titles.
const CLOUDFLARE_BLOCK_TITLE: &str = "attention required! | cloudflare";

/// Lowercase markup that, all together, marks one of Cloudflare's block pages under any title,
/// and the evidence it is.
const CLOUDFLARE_BLOCK_PAGES: &[(&[&str], &str)] = &[
    (
        &["id=\"cf-error-details\"", "sorry, you have been blocked"],
        "cloudflare_block_page",
    ),
    // Error 1020 "Access denied", the answer to a request a firewall rule denies, in Cloudflare's
    // classic error template and in its current one. Its other errors share both templates and
    // the "Access denied" title, rate limiting (1015) among them, so only the code proves a block.
    (
        &["id=\"cf-error-details\"", CLOUDFLARE_ACCESS_DENIED_CODE],
        "cloudflare_error_1020",
    ),
    (
        &["class=\"cf-error-title\"", CLOUDFLARE_ACCESS_DENIED_CODE],
        "cloudflare_error_1020",
    ),
];

/// The error code Cloudflare's error templates show for a request a firewall rule denied.
const CLOUDFLARE_ACCESS_DENIED_CODE: &str = "<span>1020</span>";

/// Whether a response with `status` can be a challenge: shops serve them as a success or as 403,
/// and AWS WAF its captcha as 405. Other statuses keep their own meaning, such as 429 and 503
/// asking for less traffic.
pub fn may_be_challenge_status(status: u16) -> bool {
    (200..300).contains(&status) || status == 403 || status == AWS_WAF_CAPTCHA_STATUS
}

/// The challenge a response with `status`, `headers` and `body` is, or `None` for every response
/// that is not provably one.
pub fn detect_bot_challenge(status: u16, headers: &HeaderMap, body: &[u8]) -> Option<BotChallenge> {
    if !may_be_challenge_status(status) {
        return None;
    }
    challenge_from_headers(headers)
        .or_else(|| challenge_from_body(body))
        .filter(|challenge| {
            status != AWS_WAF_CAPTCHA_STATUS || challenge.vendor == ChallengeVendor::AwsWaf
        })
}

/// The firewall block page a `403` with `body` is, or `None` for every response that is not
/// provably one. A page that is a challenge, which can be passed, is never a block.
pub fn detect_hard_block(status: u16, body: &[u8]) -> Option<HardBlock> {
    if status != HARD_BLOCK_STATUS {
        return None;
    }
    let text = small_page_text(body)?;
    if challenge_from_text(&text).is_some() {
        return None;
    }
    cloudflare_block_evidence(&text).map(|evidence| HardBlock {
        vendor: ChallengeVendor::Cloudflare,
        evidence,
    })
}

/// The evidence that lowercase page `text` is one of Cloudflare's block pages.
fn cloudflare_block_evidence(text: &str) -> Option<&'static str> {
    if normalized_title(text).as_deref() == Some(CLOUDFLARE_BLOCK_TITLE) {
        return Some("cloudflare_block_title");
    }
    CLOUDFLARE_BLOCK_PAGES
        .iter()
        .find(|(markers, _)| markers.iter().all(|marker| text.contains(marker)))
        .map(|(_, evidence)| *evidence)
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

/// The lowercase text of `body`, when it is small enough to be a challenge or block page.
fn small_page_text(body: &[u8]) -> Option<String> {
    (body.len() <= MAX_CHALLENGE_BODY_BYTES).then(|| String::from_utf8_lossy(body).to_lowercase())
}

fn challenge_from_body(body: &[u8]) -> Option<BotChallenge> {
    challenge_from_text(&small_page_text(body)?)
}

/// The challenge lowercase page `text` is by its markup or title.
fn challenge_from_text(text: &str) -> Option<BotChallenge> {
    if let Some((_, vendor, evidence)) = CHALLENGE_BODY_MARKERS
        .iter()
        .find(|(marker, _, _)| text.contains(marker))
    {
        return Some(BotChallenge {
            vendor: *vendor,
            evidence,
        });
    }
    let title = normalized_title(text)?;
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

    /// AWS WAF's captcha page, served with 405 when a rule's action is CAPTCHA.
    const AWS_WAF_CAPTCHA: &str = r#"<!DOCTYPE html><html lang="en"><head><meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1"><title></title>
<script>window.awsWafCookieDomainList = [];
window.gokuProps = {"key":"AQIDAHjcYu/GjX+QlghicBgQ/7bFaQZ+m5FKCMDnO+vTbNg96AE","iv":"CgAHJCJ+eAAAAQBk","context":"d2bE5d0Zp1"};</script>
<script src="https://3cc1b8a2c1c5.edge.captcha-sdk.awswaf.com/3cc1b8a2c1c5/jsapi.js"></script></head>
<body><div id="captcha-container"></div>
<script>AwsWafCaptcha.renderCaptcha(document.getElementById("captcha-container"), {apiKey: window.gokuProps.key});</script>
</body></html>"#;

    /// Cloudflare's "Sorry, you have been blocked" page for a request its firewall refused.
    const CLOUDFLARE_BLOCK: &str = r#"<!DOCTYPE html><html class="no-js" lang="en-US"><head>
<title>Attention Required! | Cloudflare</title><meta charset="UTF-8" />
<link rel="stylesheet" id="cf_styles-css" href="/cdn-cgi/styles/cf.errors.css" /></head>
<body><div id="cf-wrapper"><div id="cf-error-details" class="cf-error-details-wrapper">
<h1 data-translate="block_headline">Sorry, you have been blocked</h1>
<h2 class="cf-subheadline"><span data-translate="unable_to_access">You are unable to access</span> shop.example</h2>
<p>This website is using a security service to protect itself from online attacks.</p>
<span class="cf-footer-item">Cloudflare Ray ID: <strong>8f0a1b2c3d4e5f60</strong></span></div></div></body></html>"#;

    /// Cloudflare's error 1020 page in its current template, for a request a firewall rule
    /// denied. It shows the client's IP.
    const CLOUDFLARE_ERROR_1020: &str = r#"<!DOCTYPE html>
<html lang="en-US">
<head>
	<title>Access denied | shop.example | Cloudflare</title>
	<meta http-equiv="X-UA-Compatible" content="IE=Edge">
	<meta name="robots" content="noindex, nofollow">
	<link rel="stylesheet" href="/cdn-cgi/styles/errors.css" media="screen">
	<script>a = {event: "feedback clicked", properties: {errorCode: 1020}, helpful: a, version: 1};</script>
	<script defer="" src="https://performance.radar.cloudflare.com/beacon.js"></script>
</head>
<body>
	<div class="cf-main-wrapper" role="main">
		<div class="cf-header cf-section">
			<div class="cf-error-title">
				<h1>Access denied</h1>
				<span class="cf-code-label">Error code <span>1020</span></span>
			</div>
			<div class="cf-error-description">
				<p>You do not have access to shop.example.</p>
				<p>The site owner may have set restrictions that prevent you from accessing the site.</p>
			</div>
		</div>
	</div>
	<div class="cf-details-wrapper">
		<div class="cf-error-wrapper" id="error-details">
			<p class="cf-error-details-endpoint">I got an error when visiting shop.example/.</p>
			<p>Error code: 1020</p>
			<p>Ray ID: 8f0a1b2c3d4e5f60</p>
			<p>IP: 203.0.113.7</p>
		</div>
		<p>Performance &amp; security by <a rel="noopener noreferrer" href="https://www.cloudflare.com?utm_source=1020_error" target="_blank">Cloudflare</a></p>
	</div>
</body>
</html>"#;

    /// Cloudflare's error 1020 page in its classic template, which its other errors share.
    const CLOUDFLARE_CLASSIC_ERROR_1020: &str = r#"<!DOCTYPE html>
<html class="no-js" lang="en-US"> <!--<![endif]-->
<head>
<title>Access denied | shop.example used Cloudflare to restrict access</title>
<meta charset="UTF-8" />
<meta name="robots" content="noindex, nofollow" />
<link rel="stylesheet" id="cf_styles-css" href="/cdn-cgi/styles/main.css" />
</head>
<body>
<div id="cf-wrapper">
    <div id="cf-error-details" class="p-0">
        <header class="mx-auto pt-10 lg:pt-6 lg:px-8 w-240 lg:w-full mb-15 antialiased">
            <h1 class="inline-block md:block mr-2 md:mb-2 font-light text-60 md:text-3xl text-black-dark leading-tight">
              <span data-translate="error">Error</span>
              <span>1020</span>
            </h1>
            <span class="inline-block md:block heading-ray-id font-mono text-15 lg:text-sm lg:leading-relaxed">Ray ID: 8f0a1b2c3d4e5f60 &bull;</span>
            <h2 class="text-gray-600 leading-1.3 text-3xl lg:text-2xl font-light">Access denied</h2>
        </header>
        <section class="w-240 lg:w-full mx-auto mb-8 lg:px-8">
            <div id="what-happened-section" class="w-1/2 md:w-full">
                <h2 class="text-3xl leading-tight font-normal mb-4 text-black-dark antialiased" data-translate="what_happened">What happened?</h2>
                <p>This website is using a security service to protect itself from online attacks.</p>
            </div>
        </section>
        <span class="cf-footer-item sm:block sm:mb-1">Cloudflare Ray ID: <strong class="font-semibold">8f0a1b2c3d4e5f60</strong></span>
        <span class="hidden" id="cf-footer-ip">203.0.113.7</span>
    </div>
</div>
</body>
</html>"#;

    /// The headers Cloudflare serves its error pages with, which name no challenge.
    const CLOUDFLARE_ERROR_HEADERS: [(&str, &str); 3] = [
        ("server", "cloudflare"),
        ("cf-ray", "8f0a1b2c3d4e5f60-FRA"),
        ("content-type", "text/html; charset=UTF-8"),
    ];

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

            let device_check = page
                .replace("'rt':'c'", "'rt':'i'")
                .replace("/c.js", "/i.js");
            assert_eq!(
                detect(403, &[], &device_check).map(|challenge| challenge.vendor),
                Some(ChallengeVendor::DataDome)
            );
        }

        #[test]
        fn should_recognise_a_bot_check_page_served_with_200_by_its_exact_title() {
            let challenge = detect(200, &[], GERMAN_BOT_CHECK).unwrap();
            assert_eq!(challenge.vendor, ChallengeVendor::Unattributed);
            assert_eq!(challenge.evidence, "interstitial_title");
        }

        #[test]
        fn should_recognise_an_aws_waf_captcha_answered_with_405() {
            let by_header = detect(405, &[("x-amzn-waf-action", "captcha")], "").unwrap();
            assert_eq!(by_header.vendor, ChallengeVendor::AwsWaf);
            assert_eq!(by_header.evidence, "aws_waf_action_header");

            let by_body = detect(405, &[("server", "awselb/2.0")], AWS_WAF_CAPTCHA).unwrap();
            assert_eq!(by_body.vendor, ChallengeVendor::AwsWaf);
            assert_eq!(by_body.evidence, "aws_waf_challenge_page");
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
<meta http-equiv="Content-Security-Policy" content="script-src 'self' https://ct.captcha-delivery.com; frame-src https://geo.captcha-delivery.com">
<link rel="preconnect" href="https://geo.captcha-delivery.com">
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
        fn should_not_take_an_ordinary_405_for_a_challenge() {
            let method_not_allowed = "<html><head><title>405 Method Not Allowed</title></head>\
<body><h1>Method Not Allowed</h1></body></html>";
            assert_eq!(detect(405, &[("allow", "POST")], method_not_allowed), None);
            assert_eq!(detect(405, &[("x-amzn-waf-action", "allow")], ""), None);
        }

        #[test]
        fn should_read_only_aws_waf_evidence_on_a_405() {
            assert_eq!(
                detect(405, &[("cf-mitigated", "challenge")], CLOUDFLARE_CHALLENGE),
                None
            );
            assert_eq!(detect(405, &[], GERMAN_BOT_CHECK), None);
            assert_eq!(detect(405, &[], SITEGROUND_CAPTCHA), None);
        }

        #[test]
        fn should_not_take_an_aws_waf_protected_page_for_a_challenge() {
            let page = r#"<html><head><title>Walnut Bureau</title>
<script type="text/javascript" src="https://3cc1b8a2c1c5.edge.sdk.awswaf.com/3cc1b8a2c1c5/challenge.js" defer></script>
<script>AwsWafIntegration.saveReferrer();</script></head><body><h1>Walnut Bureau</h1></body></html>"#;
            assert_eq!(detect(200, &[], page), None);
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

    mod hard_blocks {
        use super::*;

        #[test]
        fn should_recognise_a_cloudflare_block_page_as_a_hard_block_not_a_challenge() {
            assert_eq!(
                detect_hard_block(403, CLOUDFLARE_BLOCK.as_bytes()),
                Some(HardBlock {
                    vendor: ChallengeVendor::Cloudflare,
                    evidence: "cloudflare_block_title",
                })
            );
            assert_eq!(
                detect(403, &[("server", "cloudflare")], CLOUDFLARE_BLOCK),
                None
            );
        }

        #[test]
        fn should_recognise_a_cloudflare_block_page_under_another_title() {
            let page = CLOUDFLARE_BLOCK.replace(
                "Attention Required! | Cloudflare",
                "shop.example | Access denied",
            );
            assert_eq!(
                detect_hard_block(403, page.as_bytes()).map(|block| block.evidence),
                Some("cloudflare_block_page")
            );
        }

        /// A firewall rule's denial is no challenge either, although Spider reads the vendor in
        /// front of it as one.
        #[test]
        fn should_recognise_a_cloudflare_error_1020_page_as_a_hard_block_not_a_challenge() {
            for page in [CLOUDFLARE_ERROR_1020, CLOUDFLARE_CLASSIC_ERROR_1020] {
                assert_eq!(
                    detect_hard_block(403, page.as_bytes()),
                    Some(HardBlock {
                        vendor: ChallengeVendor::Cloudflare,
                        evidence: "cloudflare_error_1020",
                    }),
                    "{page}"
                );
                assert_eq!(detect(403, &CLOUDFLARE_ERROR_HEADERS, page), None, "{page}");
            }
        }

        /// Cloudflare's other errors share the 1020 page's templates and title; rate limiting
        /// (1015) asks the client to wait, which a hard block must not hide.
        #[test]
        fn should_not_take_another_cloudflare_error_for_a_hard_block() {
            let rate_limited = CLOUDFLARE_CLASSIC_ERROR_1020
                .replace("<span>1020</span>", "<span>1015</span>")
                .replace(">Access denied</h2>", ">You are being rate limited</h2>");
            let current_template =
                CLOUDFLARE_ERROR_1020.replace("<span>1020</span>", "<span>1015</span>");
            for page in [rate_limited, current_template] {
                assert_eq!(detect_hard_block(403, page.as_bytes()), None, "{page}");
                assert_eq!(detect_hard_block(429, page.as_bytes()), None, "{page}");
            }
        }

        #[test]
        fn should_keep_a_passable_cloudflare_challenge_a_challenge() {
            let captcha = CLOUDFLARE_CHALLENGE
                .replace("Just a moment...", "Attention Required! | Cloudflare");
            let denial_with_challenge = CLOUDFLARE_ERROR_1020.replace(
                "</head>",
                "<script>window._cf_chl_opt={cvId: '3'};</script></head>",
            );
            for page in [captcha.as_str(), denial_with_challenge.as_str()] {
                assert_eq!(detect_hard_block(403, page.as_bytes()), None, "{page}");
                assert_eq!(
                    detect(403, &[], page).map(|challenge| challenge.vendor),
                    Some(ChallengeVendor::Cloudflare),
                    "{page}"
                );
            }
            assert_eq!(
                detect_hard_block(403, CLOUDFLARE_CHALLENGE.as_bytes()),
                None
            );
        }

        #[test]
        fn should_not_take_other_answers_for_a_hard_block() {
            for status in [200, 405, 429, 503] {
                for page in [CLOUDFLARE_BLOCK, CLOUDFLARE_ERROR_1020] {
                    assert_eq!(detect_hard_block(status, page.as_bytes()), None, "{status}");
                }
            }
            let forbidden = "<html><head><title>403 Forbidden</title></head>\
<body><h1>Forbidden</h1><p>Sorry, you have been blocked from this area.</p></body></html>";
            assert_eq!(detect_hard_block(403, forbidden.as_bytes()), None);
            let shop_denial = "<html><head><title>Access denied | shop.example</title></head>\
<body><h1>Access denied</h1><p>Error code <span>1020</span>: your account is locked.</p></body></html>";
            assert_eq!(detect_hard_block(403, shop_denial.as_bytes()), None);
            let large = format!(
                "{CLOUDFLARE_BLOCK}{}",
                "<p>catalogue</p>".repeat(MAX_CHALLENGE_BODY_BYTES / 16 + 1)
            );
            assert_eq!(detect_hard_block(403, large.as_bytes()), None);
        }
    }
}

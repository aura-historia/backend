# Crawler networking

All crawler egress is fail-closed. Spider roots, scraper HTML fetches, image probes, and review live inspection accept only public HTTP(S) destinations.

## Destination policy

Before each request, the crawler:

1. accepts only HTTP or HTTPS URLs with DNS hostnames, no userinfo, no IP literals, and ports 80 or 443;
2. resolves the hostname immediately;
3. rejects absent, mixed, or non-public DNS answers; and
4. pins the approved addresses in the HTTP client for that request.

This prevents a request from following DNS rebinding to a private address. Automatic redirects are disabled; each permitted redirect is explicitly validated and bounded. Spider navigation remains confined to its configured host, while scraper navigation may use only the configured bare host or its one-`www` equivalent. A redirect outside the applicable boundary fails rather than becoming a new crawl target. The spider treats an unrelated root redirect as terminal before URL state is persisted.

Crawler domain ownership is a local canonical hostname: lowercase, no trailing dot, and at most one leading `www.` removed. Repeated `www.` prefixes and equivalent ownership by different ListingSources are rejected. The crawl-root host is retained separately so spider navigation remains exact-host constrained.

## TLS trust

Every crawler request verifies certificates against the operating system's trust store, through the platform verifier Spider's client also uses. The spider and the scraper therefore accept the same shops. A shop whose chain ends at a root the host does not trust fails as a connect error, and the logged details name the cause, for example `UnknownIssuer`.

On Linux the trust store is the system CA bundle (`SSL_CERT_FILE`/`SSL_CERT_DIR` or the distribution's default paths), read once per process. An image without a CA bundle fails closed: the crawler logs once that the trust store is unavailable, and every HTTPS request fails. Windows and macOS use the OS certificate engine, which can also accept roots that the Mozilla-derived Linux bundles do not include yet.

## Crawler identity

Every crawler request (spider pages and robots.txt, scraper HTML fetches, image probes, and review live inspection) sends the single user agent `AuraHistoriaBot/<version> (+https://aura-historia.com)`, defined once in the crawler's network module.

- The crawler identifies itself honestly instead of posing as a browser. robots.txt groups are matched against the leading `AuraHistoriaBot` token, so a shop can allow or refuse the crawler by name.
- Use the bare product-token form. Some shop firewalls reject the `Mozilla/5.0 (compatible; ...)` form from unverified addresses, and a pinned browser version goes stale and gets blocked as outdated.
- A 401 or 403 from the crawl root is reported as `AccessDenied` with the root status and the user agent in the crawl diagnostics. Spider's own reading of the refusal (a blocked status, a JavaScript challenge, a recognised vendor block) keeps precedence over the generic `crawl_root_access_denied` reason.

## robots.txt

Spider and scraper honour robots.txt through one RFC 9309 implementation in the crawler's network module:

- The group naming `AuraHistoriaBot` applies, otherwise the `*` groups; the longest matching `Allow`/`Disallow` rule decides and `Allow` wins ties.
- robots.txt redirects are followed up to five times, to any public host. A 4xx answer other than 429 (401 and 403 included), a redirect to a target the crawler may not request or too many redirects means robots.txt is unavailable and everything is allowed. A 429 or 5xx answer, or a transport failure (body read included) on a redirect target off the shop's host, means robots.txt is unreachable and everything is disallowed. A transport failure on the shop's own host fails the request or crawl like any other transport failure.
- Percent escapes match regardless of hex case, so `Disallow: /*%2b*` covers `%2B` and `%2b` alike.
- `Crawl-delay`, capped at 60 seconds, only ever raises the component's own delay between requests.

The scraper checks robots.txt per origin before every HTML request, redirect hops included. It reuses a fetched policy for one hour and an unreachable one for 15 minutes; concurrent requests to an origin share one robots.txt fetch, which counts against the fetch timeouts and is not retried inline, for a redirect hop either. `Crawl-delay` is a minimum spacing between request starts on the shop's domain, bare and `www` host together. Every request, redirect hops included, waits for that spacing and the latency throttle, and the wait does not count against the fetch timeouts: they bound network time only, so a slow shop is paced rather than timed out. Scraper and review live inspection share one fetcher, so both count against the same spacing; a live inspection gives up after 25 seconds, and a request abandoned while it waits hands its start back. A schema-matrix refresh fetches every review page at that spacing, so it runs in the background, one per review, with a deadline of 90 seconds per page; the matrix endpoint answers `202` while it runs and reports the matrix or the failed fetch on the next poll. A refused URL is not requested: it fails with `RobotsDisallowed` or `RobotsUnreachable`, receives the URL retry cooldown described in [Scheduling and retries](scheduling-and-retries.md#fetch-retries), and neither opens nor closes a domain circuit, since nothing reached the shop. A redirect to a URL robots.txt disallows is judged by its target: a non-product target is a removal, like any redirect to a non-product page.

The spider reads robots.txt once per crawl, after the root preflight. A root robots.txt disallows fails the crawl as `RobotsBlocked`; an unreachable robots.txt fails it as `ServerError`. The Spider library's own robots handling is off, and its only pre-fetch veto is a regex deny list. So the rules reach it as deny-list patterns, which are exact for `Disallow` rules and for literal-prefix `Allow` exceptions; rules that do not compile into a deny list fail the crawl as `RobotsBlocked`. Other `Allow` exceptions are not carved out, so the spider may skip more than robots.txt asks, never less. When such an exception was written for a `Disallow` (its pattern extends the `Disallow`, as Shopify's `Allow: /*/account/login` does), the crawl diagnostics count the rule and `crawler.spider.robots_rules_failing_closed` names it.

## Stored URLs

The crawler stores a page under the URL it fetched, so the scraper requests and robots.txt judges the same URL the spider saw. The only changes are removals: the fragment, credentials, trailing slashes after a non-root path, `utm_*` parameters and a query left empty. Path and query keep their percent-encoding, hex case and parameter order byte for byte; `%20`, `+` and `%2B` stay distinct, as robots.txt rules such as Shopify's `Disallow: /collections/*+*` distinguish them.

Rows stored while the crawler re-encoded queries (`%20` as `+`, sorted parameters, decoded path escapes) cannot be rewritten from the stored form alone, because `+` may have been fetched as `+` or `%20`. When the spider rediscovers such a page, its row moves to the fetched spelling and keeps its scrape state and raw-capture identity. The row stays put when the shop also links the old spelling itself or the fetched spelling already has a row; when several fetched spellings share one old spelling, the first in URL order takes the row.

## Resource rails

Spider fetches use a bounded per-request timeout plus whole-crawl bounds: at most 500,000 pages and 10 minutes. [Scheduling](scheduling-and-retries.md#spider-crawl-completion) defines how a crawl stopped by either bound is recorded. Its response limit must be supplied through `SPIDER_MAX_SIZE_BYTES`; accepted values range from 1 MiB through 8 MiB, and the limit applies to declared and streamed response bodies.

Scraper HTML and image requests also use bounded redirects, timeouts, and response bodies. An HTML fetch attempt starts from a fixed timeout and grows with the domain's smoothed response latency, to a fixed upper bound, so a shop that is slow throughout is waited for instead of timed out; the whole fetch with its retries grows by the same amount, while DNS and connect timeouts stay fixed. Image-cache results never exempt the current target from destination checks.

Stored URLs drop the trailing slash, so on a shop that redirects every path to its trailing-slash form (WordPress and WooCommerce by default) each fetch would cost a redirect. Once the scraper's fetcher sees a host redirect a path to exactly its slashed form, later fetches to that host request the slashed form of extension-less paths directly; a slashed form answering 404 or 410, or refused by robots.txt, is followed by a fetch of the stored form within the same budget. A host that redirects a slashed path back loses the preference. The preference lives in process memory only and never changes a stored URL.

These rails are safety and availability limits, not a claim that every public address is reachable from every deployment.

## Related contracts

- [Scheduling and retries](scheduling-and-retries.md) defines retry and cooldown behavior after a network result.
- [Review service operations](operations.md#review-service) defines authenticated live inspection.
- The shared [`public-network-policy`](../../src/public-network-policy/) implementation is the source for public-address classification.

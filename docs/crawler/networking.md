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

- The crawler identifies itself honestly instead of posing as a browser. Spider matches robots.txt groups against the leading `AuraHistoriaBot` token, so a shop can allow or refuse the crawler by name.
- Use the bare product-token form. Some shop firewalls reject the `Mozilla/5.0 (compatible; ...)` form from unverified addresses, and a pinned browser version goes stale and gets blocked as outdated.
- A 401 or 403 from the crawl root is reported as `AccessDenied` (`crawl_root_access_denied`) with the root status and the user agent in the crawl diagnostics, including when robots.txt was refused first. Spider's own reading of the refusal (a JavaScript challenge, a recognised vendor block) keeps precedence. A refused robots.txt on an otherwise reachable root stays `robots_txt_request_denied`.

## Resource rails

Spider fetches use a bounded per-request timeout plus whole-crawl bounds: at most 500,000 pages and 10 minutes. [Scheduling](scheduling-and-retries.md#spider-crawl-completion) defines how a crawl stopped by either bound is recorded. Its response limit must be supplied through `SPIDER_MAX_SIZE_BYTES`; accepted values range from 1 MiB through 8 MiB, and the limit applies to declared and streamed response bodies.

Scraper HTML and image requests also use bounded redirects, timeouts, and response bodies. Image-cache results never exempt the current target from destination checks.

These rails are safety and availability limits, not a claim that every public address is reachable from every deployment.

## Related contracts

- [Scheduling and retries](scheduling-and-retries.md) defines retry and cooldown behavior after a network result.
- [Review service operations](operations.md#review-service) defines authenticated live inspection.
- The shared [`public-network-policy`](../../src/public-network-policy/) implementation is the source for public-address classification.

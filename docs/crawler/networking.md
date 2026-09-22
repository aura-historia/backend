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

## Resource rails

Spider fetches use a bounded per-request timeout plus whole-crawl bounds: at most 10,000 pages and 10 minutes. Its response limit must be supplied through `SPIDER_MAX_SIZE_BYTES`; accepted values range from 1 MiB through 8 MiB, and the limit applies to declared and streamed response bodies.

Scraper HTML and image requests also use bounded redirects, timeouts, and response bodies. Image-cache results never exempt the current target from destination checks.

These rails are safety and availability limits, not a claim that every public address is reachable from every deployment.

## Related contracts

- [Scheduling and retries](scheduling-and-retries.md) defines retry and cooldown behavior after a network result.
- [Review service operations](operations.md#review-service) defines authenticated live inspection.
- The shared [`public-network-policy`](../../src/public-network-policy/) implementation is the source for public-address classification.

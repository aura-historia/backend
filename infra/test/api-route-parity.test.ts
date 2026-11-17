import { API_SELECTIVE_READ_PATH_PATTERNS } from "../src/constructs/api";
import { axumRouteKeys, swaggerRouteKeys } from "./api-route-contract";

const SELECTIVE_CACHE_GET_ROUTE_KEYS = [
  "GET /v1/listing-sources",
  "GET /v1/listing-sources/by-slug/{}",
  "GET /v1/auctions",
  "GET /v1/auctions/{}",
  "GET /v1/auctions/{}/product-listings",
  "GET /v1/product-listings",
  "GET /v1/product-listings/by-slug/{}",
  "GET /v1/product-listings/{}",
  "GET /v1/product-listings/{}/history",
  "GET /v1/product-listings/{}/similar",
  "GET /v1/overview",
  "GET /.well-known/api-catalog",
  "HEAD /.well-known/api-catalog",
];
const SITE_DISCOVERY_ROUTE_KEYS = [
  "ai-catalog.json",
  "poppy.json",
  "agent-skills/index.json",
  "agent-skills/{}/SKILL.md",
  "auth.md",
  "oauth/webmcp-client.json",
].flatMap((document) => [`GET /v1/site-discovery/${document}`, `HEAD /v1/site-discovery/${document}`]);
const WELL_KNOWN_ROUTE_KEYS = [
  "GET /.well-known/oauth-authorization-server",
  "GET /.well-known/oauth-protected-resource",
  "GET /.well-known/api-catalog",
  "HEAD /.well-known/api-catalog",
];

/** Canonical routes are `/v1` operations, exact well-known documents or site discovery `HEAD`s. */
function isCanonicalRouteKey(key: string): boolean {
  return /^(GET|POST|PUT|PATCH|DELETE) \/v1\//.test(key) || WELL_KNOWN_ROUTE_KEYS.includes(key) ||
    SITE_DISCOVERY_ROUTE_KEYS.includes(key);
}

/** CloudFront path patterns: `*` matches any characters, slashes included; `?` matches one. */
function matchesPathPattern(pattern: string, path: string): boolean {
  const expression = pattern.replace(/[.+^${}()|[\]\\]/g, "\\$&").replaceAll("*", ".*").replaceAll("?", ".");
  return new RegExp(`^${expression}$`).test(path);
}

function selectiveRouteKeys(routes: readonly string[]): string[] {
  return routes.filter((key) => {
    const path = key.slice(key.indexOf(" ") + 1).replaceAll("{}", "x1");
    return API_SELECTIVE_READ_PATH_PATTERNS.some((pattern) => matchesPathPattern(pattern, path));
  }).sort();
}

describe("REST route contract", () => {
  test("Axum serves exactly the OpenAPI operations, canonical and without aliases or catch-alls", () => {
    const swagger = swaggerRouteKeys();
    const axum = axumRouteKeys();

    for (const routes of [swagger, axum]) {
      expect(new Set(routes).size).toBe(routes.length);
      expect(routes.filter((key) => !isCanonicalRouteKey(key))).toEqual([]);
      expect(routes.filter((key) => /[*+]/.test(key))).toEqual([]);
      expect(routes.filter((key) => key.includes(" /api/"))).toEqual([]);
    }
    expect(axum).toEqual(swagger);
  });

  test("CloudFront shares only the reviewed anonymous GET and HEAD representations", () => {
    const selective = selectiveRouteKeys(swaggerRouteKeys());
    expect(selective).toEqual([...SELECTIVE_CACHE_GET_ROUTE_KEYS, ...SITE_DISCOVERY_ROUTE_KEYS].sort());
    // The selective behaviors admit only GET, HEAD and OPTIONS, so no write may share their paths.
    expect(selective.filter((key) => !/^(GET|HEAD) /.test(key))).toEqual([]);
  });

  test("keeps management, account, OAuth, probe, webhook and consent families out of shared caching", () => {
    const selective = selectiveRouteKeys(swaggerRouteKeys()).map((key) => key.slice(key.indexOf(" ") + 1));
    for (const family of ["/v1/me", "/v1/admin", "/v1/oauth", "/v1/health", "/v1/ready", "/v1/webhooks", "/v1/newsletter-subscriptions"]) {
      expect(selective.filter((path) => path === family || path.startsWith(`${family}/`))).toEqual([]);
    }
    expect(selective.filter((path) => path.startsWith("/v1/listing-sources/") && path.includes("/product-listings"))).toEqual([]);
  });
});

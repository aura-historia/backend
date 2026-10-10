import * as cdk from "aws-cdk-lib";
import { Template } from "aws-cdk-lib/assertions";
import * as fs from "node:fs";
import * as path from "node:path";
import { createApplicationStacks } from "../src/application-stack";
import {
  API_ROUTE_CATALOG,
  OAuthCredentialRequirement,
  ProviderProofRequirement,
  RouteAuthPolicy,
  RouteAuthorizationClass,
} from "../src/constructs/api";
import { STAGES, stageConfig, type StageName } from "../src/config";

type CloudFormationResource = {
  readonly Properties: Record<string, unknown>;
};

const ASYNC_PATH = "/api/v1/listing-sources/{listingSourceId}/product-listings/async";
const SELECTIVE_CACHE_GET_ROUTE_KEYS = [
  "GET /api/v1/listing-sources",
  "GET /api/v1/listing-sources/by-slug/{}",
  "GET /api/v1/auctions",
  "GET /api/v1/auctions/{}",
  "GET /api/v1/auctions/{}/product-listings",
  "GET /api/v1/product-listings",
  "GET /api/v1/product-listings/by-slug/{}",
  "GET /api/v1/product-listings/{}",
  "GET /api/v1/product-listings/{}/history",
  "GET /api/v1/product-listings/{}/similar",
];

const RUST_ROUTE_FILES = [
  "../../src/aura-historia-api/src/lib.rs",
  "../../src/aura-historia-api/src/search_filters/mod.rs",
  "../../src/aura-historia-api/src/notifications/mod.rs",
  "../../src/aura-historia-api/src/partnership_applications/mod.rs",
  "../../src/aura-historia-api/src/partnerships/mod.rs",
] as const;

function normalizePath(value: string): string {
  return value.replace(/\{[^}]+}/g, "{}");
}

function routeKey(method: string, routePath: string): string {
  return `${method.toUpperCase()} ${normalizePath(routePath)}`;
}

function matchesSelectiveCacheBehavior(path: string): boolean {
  return (
    path === "/api/v1/listing-sources" ||
    path.startsWith("/api/v1/listing-sources/by-slug/") ||
    path === "/api/v1/auctions" ||
    path.startsWith("/api/v1/auctions/") ||
    path === "/api/v1/product-listings" ||
    path.startsWith("/api/v1/product-listings/")
  );
}

function catalogRouteKeys(): string[] {
  return API_ROUTE_CATALOG.map((definition) => routeKey(definition.method, definition.path)).sort();
}

function swaggerRouteKeys(): string[] {
  const lines = fs.readFileSync(path.join(__dirname, "../../docs/swagger.yaml"), "utf8").split("\n");
  const routes: string[] = [];
  let currentPath: string | undefined;

  for (const line of lines) {
    const routeMatch = line.match(/^  (\/[^:]+):\s*$/);
    if (routeMatch) {
      currentPath = routeMatch[1];
      continue;
    }
    const methodMatch = line.match(/^    (get|post|put|patch|delete):\s*$/);
    if (methodMatch && currentPath) {
      routes.push(routeKey(methodMatch[1], currentPath));
    }
  }

  return routes.sort();
}

function axumRouteKeys(): string[] {
  const routes = new Set<string>();

  for (const fileName of RUST_ROUTE_FILES) {
    const source = fs.readFileSync(path.join(__dirname, fileName), "utf8");
    const routeStart = /\.route\(\s*"([^"]+)"\s*,/g;
    let match: RegExpExecArray | null;
    while ((match = routeStart.exec(source))) {
      const routePath = match[1];
      if (!routePath.startsWith("/api/v1/")) {
        continue;
      }
      const nextRoute = source.indexOf(".route(", routeStart.lastIndex);
      const stateBoundary = source.indexOf(".with_state", routeStart.lastIndex);
      const handlerEnd = [nextRoute, stateBoundary].filter((index) => index !== -1).reduce(
        (end, index) => Math.min(end, index),
        source.length,
      );
      const handlers = source.slice(routeStart.lastIndex, handlerEnd);
      for (const method of handlers.matchAll(/\b(get|post|put|patch|delete)\s*\(/g)) {
        routes.add(routeKey(method[1], routePath));
      }
    }
  }

  return [...routes].sort();
}

function apiTemplate(stage: StageName): Template {
  const app = new cdk.App({ analyticsReporting: false });
  return Template.fromStack(createApplicationStacks(app, { stage }).api);
}

function resolveConcreteArn(value: unknown): string {
  if (typeof value === "string") {
    return value;
  }
  const join = (value as { "Fn::Join": [string, unknown[]] })["Fn::Join"];
  expect(join[0]).toBe("");
  return join[1].map((part) => {
    if (typeof part === "string") {
      return part;
    }
    expect(part).toEqual({ Ref: "AWS::Partition" });
    return "aws";
  }).join("");
}

describe("HTTP API route policy matrix", () => {
  test("covers the declared Axum and OpenAPI operations without a catch-all", () => {
    const catalog = catalogRouteKeys();
    const swagger = swaggerRouteKeys();
    const axum = axumRouteKeys();

    expect(catalog).toHaveLength(112);
    expect(new Set(catalog).size).toBe(catalog.length);
    expect(catalog).toEqual(swagger);
    expect(catalog).toEqual(axum);
    expect(catalog).not.toContain("ANY /{proxy+}");
  });

  test("keeps newsletter request optional-bearer and confirmation anonymous", () => {
    expect(API_ROUTE_CATALOG.filter((route) => route.path.startsWith("/api/v1/newsletter-subscriptions")))
      .toEqual([
        {
          method: "PUT",
          path: "/api/v1/newsletter-subscriptions",
          lambda: "auraHistoriaApi",
          auth: RouteAuthPolicy.OptionalBearer,
          policy: {
            bearer: "OPTIONAL",
            authorization: RouteAuthorizationClass.Public,
            oauthCredentials: OAuthCredentialRequirement.None,
            providerProof: ProviderProofRequirement.None,
          },
        },
        {
          method: "POST",
          path: "/api/v1/newsletter-subscriptions/confirm",
          lambda: "auraHistoriaApi",
          auth: RouteAuthPolicy.Anonymous,
          policy: {
            bearer: "NONE",
            authorization: RouteAuthorizationClass.Public,
            oauthCredentials: OAuthCredentialRequirement.None,
            providerProof: ProviderProofRequirement.None,
          },
        },
      ]);
  });

  test("requires Party-location management authority and exposes no public location routes", () => {
    const protectedRoutes = API_ROUTE_CATALOG.filter((route) =>
      route.path.startsWith("/api/v1/parties/{party_id}/locations"));
    expect(protectedRoutes).toHaveLength(5);
    for (const route of protectedRoutes) {
      expect(route.auth).toBe(RouteAuthPolicy.ApplicationBearer);
      expect(route.policy).toEqual({
        bearer: "REQUIRED",
        authorization: RouteAuthorizationClass.PartyLocationManager,
        oauthCredentials: OAuthCredentialRequirement.None,
        providerProof: ProviderProofRequirement.None,
      });
      expect(matchesSelectiveCacheBehavior(route.path)).toBe(false);
    }
    const publicRoutes = API_ROUTE_CATALOG.filter((route) =>
      route.path.startsWith("/api/v1/public/parties/{party_id}/locations"));
    expect(publicRoutes).toHaveLength(0);
  });

  test("documents anonymous versioned probes with their body contracts", () => {
    const swagger = fs.readFileSync(path.join(__dirname, "../../docs/swagger.yaml"), "utf8");
    const health = swagger.split("  /api/v1/health:\n")[1]?.split(/\n  \//)[0];
    const ready = swagger.split("  /api/v1/ready:\n")[1]?.split(/\n  \//)[0];

    expect(health).toContain("security: []");
    expect(health).toContain('"200":');
    expect(health).toContain("text/plain:");
    expect(health).toContain('example: "ok\\n"');
    expect(ready).toContain("security: []");
    expect(ready).toContain('"204":');
    expect(ready).toContain('"503":');
    expect(ready).not.toContain("content:");
  });

  test("adds only POST, PATCH, PUT and DELETE on the exact async route with application bearer and Partner policy", () => {
    expect(API_ROUTE_CATALOG.filter((route) => route.path === ASYNC_PATH)).toEqual(
      ["POST", "PATCH", "PUT", "DELETE"].map((method) => ({
        method,
        path: ASYNC_PATH,
        lambda: "auraHistoriaApi",
        auth: RouteAuthPolicy.ApplicationBearer,
        policy: {
          bearer: "REQUIRED",
          authorization: RouteAuthorizationClass.Partner,
          oauthCredentials: OAuthCredentialRequirement.None,
          providerProof: ProviderProofRequirement.None,
        },
      })),
    );
    expect(API_ROUTE_CATALOG.filter((route) => route.path.endsWith("/product-listings/async")))
      .toHaveLength(4);
    expect(swaggerRouteKeys().filter((key) => key.endsWith(" /api/v1/listing-sources/{}/product-listings/async")))
      .toEqual(["DELETE", "PATCH", "POST", "PUT"].map((method) => `${method} /api/v1/listing-sources/{}/product-listings/async`));
    expect(axumRouteKeys().filter((key) => key.endsWith(" /api/v1/listing-sources/{}/product-listings/async")))
      .toEqual(["DELETE", "PATCH", "POST", "PUT"].map((method) => `${method} /api/v1/listing-sources/{}/product-listings/async`));
    expect(API_ROUTE_CATALOG.filter((route) => route.path === "/api/v1/listing-sources/{listing_source_id}/product-listings")
      .map((route) => route.method)).toEqual(["POST", "PATCH", "PUT", "DELETE"]);
  });

  test("registers provider configuration PUT routes with the Partner application bearer policy", () => {
    const routes = API_ROUTE_CATALOG.filter((route) =>
      route.path === "/api/v1/listing-sources/{listing_source_id}/ingestion-configurations/woocommerce" ||
      route.path === "/api/v1/listing-sources/{listing_source_id}/ingestion-configurations/shopify",
    );

    expect(routes.map((route) => routeKey(route.method, route.path)).sort()).toEqual([
      "PUT /api/v1/listing-sources/{}/ingestion-configurations/shopify",
      "PUT /api/v1/listing-sources/{}/ingestion-configurations/woocommerce",
    ]);
    for (const route of routes) {
      expect(route.auth).toBe(RouteAuthPolicy.ApplicationBearer);
      expect(route.policy).toEqual({
        bearer: "REQUIRED",
        authorization: RouteAuthorizationClass.Partner,
        oauthCredentials: OAuthCredentialRequirement.None,
        providerProof: ProviderProofRequirement.None,
      });
    }
  });

  test("documents the shared async request key and evaluated 202 response header for all four methods", () => {
    const swagger = fs.readFileSync(path.join(__dirname, "../../docs/swagger.yaml"), "utf8");
    const asyncPath = swagger.split(`  ${ASYNC_PATH}:\n`)[1]?.split(/\n  \/[^\n]+:\n/)[0];
    expect(asyncPath).toBeDefined();
    const methods = [...asyncPath!.matchAll(/^    (post|patch|put|delete):\s*$/gm)];
    expect(methods.map((match) => match[1])).toEqual(["post", "patch", "put", "delete"]);
    for (const [index, match] of methods.entries()) {
      const operation = asyncPath!.slice(match.index, methods[index + 1]?.index);
      const admitted = operation.split('        "202":')[1]?.split(/^        "[0-9]{3}":/m)[0];
      expect(operation).toContain('$ref: "#/components/parameters/AsyncProductListingIdempotencyKey"');
      expect(operation).toContain("- BearerAuth: []");
      expect(operation).toContain("- AccessTokenAuth: []");
      expect(admitted).toContain('$ref: "#/components/headers/AsyncProductListingIdempotencyKey"');
      expect(admitted).toContain('$ref: "#/components/schemas/AsyncProductListingBatchReport"');
    }
  });

  test("models the application-owned authorization and compound credential contracts", () => {
    expect(API_ROUTE_CATALOG.filter((route) => route.auth === RouteAuthPolicy.OptionalBearer)).not.toHaveLength(0);
    expect(API_ROUTE_CATALOG.filter((route) => route.auth === RouteAuthPolicy.ApplicationBearer)).not.toHaveLength(0);
    expect(API_ROUTE_CATALOG.filter((route) => route.auth === RouteAuthPolicy.OAuth)).not.toHaveLength(0);

    const adminRoutes = API_ROUTE_CATALOG.filter((route) => route.path.startsWith("/api/v1/admin/"));
    expect(adminRoutes.length).toBeGreaterThan(0);
    expect(adminRoutes.map((route) => routeKey(route.method, route.path)).sort()).toEqual(
      API_ROUTE_CATALOG.filter((route) => route.policy.authorization === RouteAuthorizationClass.Administrator)
        .map((route) => routeKey(route.method, route.path)).sort(),
    );
    for (const route of adminRoutes) {
      expect(route.auth).toBe(RouteAuthPolicy.ApplicationBearer);
      expect(route.policy).toEqual({
        bearer: "REQUIRED",
        authorization: RouteAuthorizationClass.Administrator,
        oauthCredentials: OAuthCredentialRequirement.None,
        providerProof: ProviderProofRequirement.None,
      });
    }

    expect(API_ROUTE_CATALOG).toContainEqual(expect.objectContaining({
      method: "POST",
      path: "/api/v1/webhooks/woocommerce/{listing_source_id}",
      policy: expect.objectContaining({
        bearer: "REQUIRED",
        authorization: RouteAuthorizationClass.Partner,
        providerProof: ProviderProofRequirement.WooCommerceSignature,
      }),
    }));
    expect(API_ROUTE_CATALOG.filter((route) => route.path === "/api/v1/webhooks/loops")).toEqual([{
      method: "POST",
      path: "/api/v1/webhooks/loops",
      lambda: "auraHistoriaApi",
      auth: RouteAuthPolicy.LoopsSignature,
      policy: {
        bearer: "NONE",
        authorization: RouteAuthorizationClass.Public,
        oauthCredentials: OAuthCredentialRequirement.None,
        providerProof: ProviderProofRequirement.LoopsSignature,
      },
    }]);
    const probeRoutes = API_ROUTE_CATALOG.filter((route) =>
      ["/api/v1/health", "/api/v1/ready", "/health", "/ready"].includes(route.path));
    expect(probeRoutes.map((route) => routeKey(route.method, route.path)).sort()).toEqual([
      "GET /api/v1/health",
      "GET /api/v1/ready",
    ]);
    expect(API_ROUTE_CATALOG).toContainEqual(expect.objectContaining({
      method: "GET",
      path: "/api/v1/health",
    }));
    expect(API_ROUTE_CATALOG).toContainEqual(expect.objectContaining({
      method: "GET",
      path: "/api/v1/ready",
    }));
    expect(API_ROUTE_CATALOG.map((route) => routeKey(route.method, route.path))).not.toContain("GET /health");
    expect(API_ROUTE_CATALOG.map((route) => routeKey(route.method, route.path))).not.toContain("GET /ready");
    for (const route of probeRoutes) {
      expect(route.auth).toBe(RouteAuthPolicy.Anonymous);
      expect(route.policy).toEqual({
        bearer: "NONE",
        authorization: RouteAuthorizationClass.Public,
        oauthCredentials: OAuthCredentialRequirement.None,
        providerProof: ProviderProofRequirement.None,
      });
    }
    const meRoutes = API_ROUTE_CATALOG.filter((route) => route.path === "/api/v1/me" || route.path.startsWith("/api/v1/me/"));
    expect(meRoutes.map((route) => routeKey(route.method, route.path))).toContain("DELETE /api/v1/me");
    for (const route of meRoutes) {
      expect(route.auth).toBe(RouteAuthPolicy.ApplicationBearer);
      expect(route.policy).toEqual({
        bearer: "REQUIRED",
        authorization: RouteAuthorizationClass.AuthenticatedUser,
        oauthCredentials: OAuthCredentialRequirement.None,
        providerProof: ProviderProofRequirement.None,
      });
    }
    expect(API_ROUTE_CATALOG).toContainEqual(expect.objectContaining({
      path: "/api/v1/oauth/authorize",
      policy: expect.objectContaining({ bearer: "REQUIRED", oauthCredentials: OAuthCredentialRequirement.AuthorizationCodePkce }),
    }));
    expect(API_ROUTE_CATALOG).toContainEqual({
      method: "GET",
      path: "/api/v1/oauth/clients/{client_id}",
      lambda: "auraHistoriaApi",
      auth: RouteAuthPolicy.ApplicationBearer,
      policy: {
        bearer: "REQUIRED",
        authorization: RouteAuthorizationClass.AuthenticatedUser,
        oauthCredentials: OAuthCredentialRequirement.None,
        providerProof: ProviderProofRequirement.None,
      },
    });
    for (const path of ["/api/v1/oauth/token", "/api/v1/oauth/revoke", "/api/v1/oauth/introspect"]) {
      expect(API_ROUTE_CATALOG).toContainEqual(expect.objectContaining({
        path,
        policy: expect.objectContaining({ bearer: "NONE", oauthCredentials: OAuthCredentialRequirement.ClientCredentials }),
      }));
    }
  });

  test("limits selective CloudFront caching to the reviewed optional-bearer GET routes", () => {
    const selectivePathRoutes = API_ROUTE_CATALOG.filter((route) =>
      matchesSelectiveCacheBehavior(route.path),
    );
    const selectiveReadRouteKeys = selectivePathRoutes
      .map((route) => routeKey(route.method, route.path))
      .sort();

    expect(selectivePathRoutes.every((route) => route.method === "GET")).toBe(true);
    expect(selectiveReadRouteKeys).toEqual([...SELECTIVE_CACHE_GET_ROUTE_KEYS].sort());
    expect(selectivePathRoutes).toHaveLength(10);
    const selectiveReadRoutes = selectivePathRoutes;
    for (const route of selectiveReadRoutes) {
      expect(route.auth).toBe(RouteAuthPolicy.OptionalBearer);
      expect(route.policy).toEqual({
        bearer: "OPTIONAL",
        authorization: RouteAuthorizationClass.Public,
        oauthCredentials: OAuthCredentialRequirement.None,
        providerProof: ProviderProofRequirement.None,
      });
    }

    const excludedFamilies = [
      "/api/v1/me",
      "/api/v1/admin",
      "/api/v1/oauth",
      "/api/v1/health",
      "/api/v1/ready",
      "/api/v1/webhooks",
      "/api/v1/newsletter-subscriptions",
    ];
    for (const family of excludedFamilies) {
      expect(selectiveReadRoutes.some((route) => route.path === family || route.path.startsWith(`${family}/`))).toBe(false);
    }
    expect(selectiveReadRoutes.some((route) =>
      route.path.startsWith("/api/v1/listing-sources/") && route.path.includes("/product-listings"),
    )).toBe(false);

    const wildcardFamilyGetRouteKeys = API_ROUTE_CATALOG.filter((route) => route.method === "GET" && (
      route.path.startsWith("/api/v1/auctions/") || route.path.startsWith("/api/v1/product-listings/")
    )).map((route) => routeKey(route.method, route.path)).sort();
    expect(wildcardFamilyGetRouteKeys).toEqual(
      SELECTIVE_CACHE_GET_ROUTE_KEYS.filter((key) =>
        key.startsWith("GET /api/v1/auctions/") || key.startsWith("GET /api/v1/product-listings/"),
      ).sort(),
    );
  });

  test.each(STAGES)("synthesizes the complete %s route matrix to one live API alias", (stage) => {
    const template = apiTemplate(stage);
    const routes = Object.values(template.findResources("AWS::ApiGatewayV2::Route")) as CloudFormationResource[];
    const integrationIds = Object.keys(template.findResources("AWS::ApiGatewayV2::Integration"));
    const integrations = Object.values(template.findResources("AWS::ApiGatewayV2::Integration")) as CloudFormationResource[];
    const permissions = Object.values(template.findResources("AWS::Lambda::Permission")) as CloudFormationResource[];
    const apiIds = Object.keys(template.findResources("AWS::ApiGatewayV2::Api"));

    expect(apiIds).toHaveLength(1);
    expect(routes.map((route) => {
      const [method, ...pathParts] = String(route.Properties.RouteKey).split(" ");
      return routeKey(method, pathParts.join(" "));
    }).sort()).toEqual(catalogRouteKeys());
    expect(routes).toHaveLength(112);
    for (const method of ["POST", "PATCH", "PUT", "DELETE"]) {
      expect(routes.filter((route) => route.Properties.RouteKey === `${method} ${ASYNC_PATH}`))
        .toEqual([expect.objectContaining({ Properties: expect.objectContaining({
          AuthorizationType: "NONE",
          Target: expect.objectContaining({ "Fn::Join": expect.arrayContaining([expect.arrayContaining([{ Ref: integrationIds[0] }])]) }),
        }) })]);
    }
    expect(routes.every((route) => route.Properties.RouteKey !== "$default" && !String(route.Properties.RouteKey).includes("/{proxy+}"))).toBe(true);
    expect(routes.every((route) => route.Properties.AuthorizationType === "NONE")).toBe(true);
    expect(Object.values(template.findResources("AWS::ApiGatewayV2::Authorizer"))).toHaveLength(0);
    expect(integrations).toHaveLength(1);
    expect(integrations[0].Properties).toEqual(expect.objectContaining({
      PayloadFormatVersion: "2.0",
      IntegrationType: "AWS_PROXY",
    }));
    expect(integrations[0].Properties).not.toHaveProperty("RequestParameters");
    expect(integrations[0].Properties).not.toHaveProperty("ResponseParameters");
    expect(routes.every((route) => JSON.stringify(route.Properties.Target).includes(integrationIds[0]))).toBe(true);
    expect(JSON.stringify(integrations[0].Properties.IntegrationUri)).toContain(`:function:aura-historia-api-${stage}:live`);
    expect(JSON.stringify(integrations[0].Properties.IntegrationUri)).not.toContain(":function/");
    expect(permissions).toHaveLength(1);
    expect(integrations[0].Properties.IntegrationUri).toEqual(permissions[0].Properties.FunctionName);
    // A single scoped statement leaves ample headroom under Lambda's 20 KiB policy limit.
    expect(Buffer.byteLength(JSON.stringify(permissions[0]))).toBeLessThan(10_000);
    expect(permissions[0].Properties).toEqual({
      Action: "lambda:InvokeFunction",
      FunctionName: {
        "Fn::Join": ["", [
          "arn:", { Ref: "AWS::Partition" }, ":lambda:", { Ref: "AWS::Region" }, ":",
          { Ref: "AWS::AccountId" }, `:function:aura-historia-api-${stage}:live`,
        ]],
      },
      Principal: "apigateway.amazonaws.com",
      SourceArn: {
        "Fn::Join": ["", [
          "arn:", { Ref: "AWS::Partition" }, ":execute-api:", { Ref: "AWS::Region" }, ":",
          { Ref: "AWS::AccountId" }, ":", { Ref: apiIds[0] }, "/*/*/*",
        ]],
      },
    });
  });

  test.each(STAGES)("allows Idempotency-Key requests and exposes the response header in %s Gateway CORS", (stage) => {
    const [api] = Object.values(apiTemplate(stage).findResources("AWS::ApiGatewayV2::Api")) as CloudFormationResource[];
    const cors = api.Properties.CorsConfiguration as Record<string, unknown>;

    expect(cors.AllowHeaders).toEqual(expect.arrayContaining(["Authorization", "Content-Type", "Idempotency-Key", "X-Correlation-Id"]));
    expect(cors.AllowMethods).toEqual(["*"]);
    expect(cors.AllowOrigins).toEqual(stageConfig(stage).apiCorsAllowOrigins);
    expect(cors.ExposeHeaders).toEqual(["Idempotency-Key"]);
  });

  test("partitions identical discovery requests across two allowed production CORS origins", () => {
    const template = apiTemplate("prod");
    const [api] = Object.values(template.findResources("AWS::ApiGatewayV2::Api")) as CloudFormationResource[];
    const [cachePolicy] = Object.values(template.findResources("AWS::CloudFront::CachePolicy")) as CloudFormationResource[];
    const cors = api.Properties.CorsConfiguration as { readonly AllowOrigins: string[] };
    const cachePolicyConfig = cachePolicy.Properties.CachePolicyConfig as {
      readonly ParametersInCacheKeyAndForwardedToOrigin: {
        readonly HeadersConfig: { readonly Headers: string[] };
      };
    };
    const allowedOrigins = ["https://aura-historia.com", "https://admin.shopify.com"];
    const pathAndQuery = "/api/v1/listing-sources?query=source";
    const cacheKeys = allowedOrigins.map((origin) => ({ pathAndQuery, origin }));

    for (const origin of allowedOrigins) {
      expect(cors.AllowOrigins).toContain(origin);
    }
    expect(cachePolicyConfig.ParametersInCacheKeyAndForwardedToOrigin.HeadersConfig.Headers).toContain("Origin");
    expect(cacheKeys[0].pathAndQuery).toBe(cacheKeys[1].pathAndQuery);
    expect(cacheKeys[0].origin).not.toBe(cacheKeys[1].origin);
  });

  test.each(["dev", "prod"] as const)("imports the %s compute alias ARN for integration and permission", (stage) => {
    const app = new cdk.App({ analyticsReporting: false });
    const stacks = createApplicationStacks(app, {
      stage,
      env: { account: "123456789012", region: "eu-central-1" },
    });
    const compute = Template.fromStack(stacks.compute);
    const api = Template.fromStack(stacks.api);
    const [alias] = Object.values(compute.findResources("AWS::Lambda::Alias")) as CloudFormationResource[];
    const functionId = (alias.Properties.FunctionName as { Ref: string }).Ref;
    const functionResource = compute.findResources("AWS::Lambda::Function")[functionId] as CloudFormationResource;
    const qualifiedArn = `arn:aws:lambda:eu-central-1:123456789012:function:${functionResource.Properties.FunctionName}:${alias.Properties.Name}`;
    const [integration] = Object.values(api.findResources("AWS::ApiGatewayV2::Integration")) as CloudFormationResource[];
    const [permission] = Object.values(api.findResources("AWS::Lambda::Permission")) as CloudFormationResource[];

    expect(alias.Properties.Name).toBe("live");
    expect(functionResource.Properties.FunctionName).toBe(`aura-historia-api-${stage}`);
    expect(qualifiedArn).toBe(`arn:aws:lambda:eu-central-1:123456789012:function:aura-historia-api-${stage}:live`);
    expect(resolveConcreteArn(integration.Properties.IntegrationUri)).toBe(qualifiedArn);
    expect(resolveConcreteArn(permission.Properties.FunctionName)).toBe(qualifiedArn);
    expect(qualifiedArn).not.toContain(":function/");
  });

  test.each([
    ["dev", "api.stage.aura-historia.com"],
    ["prod", "api.aura-historia.com"],
  ] as const)("serves %s on the exact public API host through the regional front door", (stage, host) => {
    const config = stageConfig(stage);
    const template = apiTemplate(stage);
    const [[domainId, domain]] = Object.entries(template.findResources("AWS::ApiGatewayV2::DomainName")) as [string, CloudFormationResource][];
    const [mapping] = Object.values(template.findResources("AWS::ApiGatewayV2::ApiMapping")) as CloudFormationResource[];
    const [distribution] = Object.values(template.findResources("AWS::CloudFront::Distribution")) as CloudFormationResource[];
    const [[apiId, api]] = Object.entries(template.findResources("AWS::ApiGatewayV2::Api")) as [string, CloudFormationResource][];

    expect(config.stage).toBe(stage);
    expect(config.apiDomainName).toBe(host);
    expect(config.apiEndpointUrl).toBe(`https://${host}`);
    expect(config.apiCloudFrontAliases).toEqual([host]);
    expect(config.apiGatewayCertificateArn).toBe(`{{resolve:ssm:/certificates/${stage}/api-regional-certificate-arn}}`);
    expect(config.apiCloudFrontCertificateArn).toBe(`{{resolve:ssm:/certificates/${stage}/api-cloudfront-certificate-arn}}`);
    expect(template.toJSON().Outputs.ApiGatewayEndpointUrl.Value).toBe(`https://${host}`);
    expect(api.Properties.DisableExecuteApiEndpoint).toBe(true);
    expect(domain.Properties).toEqual(expect.objectContaining({
      DomainName: host,
      DomainNameConfigurations: [expect.objectContaining({
        CertificateArn: config.apiGatewayCertificateArn,
        EndpointType: "REGIONAL",
        SecurityPolicy: "TLS_1_2",
      })],
    }));
    expect(mapping.Properties).toEqual(expect.objectContaining({
      ApiId: { Ref: apiId },
      DomainName: { Ref: domainId },
      Stage: stage,
    }));
    expect(distribution.Properties.DistributionConfig).toEqual(expect.objectContaining({
      Aliases: [host],
      Origins: [expect.objectContaining({
        DomainName: { "Fn::GetAtt": [domainId, "RegionalDomainName"] },
        CustomOriginConfig: expect.objectContaining({ OriginProtocolPolicy: "https-only" }),
      })],
      ViewerCertificate: expect.objectContaining({ AcmCertificateArn: config.apiCloudFrontCertificateArn }),
      DefaultCacheBehavior: expect.objectContaining({ OriginRequestPolicyId: "216adef6-5c7f-47e4-b989-5492eafa07d3" }),
      CacheBehaviors: expect.arrayContaining([
        expect.objectContaining({ PathPattern: "/api/v1/listing-sources", OriginRequestPolicyId: "216adef6-5c7f-47e4-b989-5492eafa07d3" }),
        expect.objectContaining({ PathPattern: "/api/*", OriginRequestPolicyId: "216adef6-5c7f-47e4-b989-5492eafa07d3" }),
      ]),
    }));
  });


  test("OpenAPI advertises the stage API host without changing production", () => {
    const swagger = fs.readFileSync(path.join(__dirname, "../../docs/swagger.yaml"), "utf8");
    expect(swagger).toContain("url: https://api.stage.aura-historia.com");
    expect(swagger).toContain("url: https://api.aura-historia.com");
    expect(swagger).not.toContain("api.dev.aura-historia.com");
  });

  test.each(["dev", "prod"] as const)("synthesizes %s selective CloudFront caching without edge auth", (stage) => {
    const template = apiTemplate(stage);
    const [[cachePolicyId, cachePolicy]] = Object.entries(template.findResources("AWS::CloudFront::CachePolicy")) as [string, CloudFormationResource][];
    const [[responseHeadersPolicyId, responseHeadersPolicy]] = Object.entries(
      template.findResources("AWS::CloudFront::ResponseHeadersPolicy"),
    ) as [string, CloudFormationResource][];
    const [[distributionId, distribution]] = Object.entries(
      template.findResources("AWS::CloudFront::Distribution"),
    ) as [string, CloudFormationResource][];
    const domains = Object.values(template.findResources("AWS::ApiGatewayV2::DomainName")) as CloudFormationResource[];
    const mappings = Object.values(template.findResources("AWS::ApiGatewayV2::ApiMapping")) as CloudFormationResource[];
    const distributionConfig = distribution.Properties.DistributionConfig as {
      readonly CacheBehaviors: Record<string, unknown>[];
      readonly CustomErrorResponses: Record<string, unknown>[];
      readonly DefaultCacheBehavior: Record<string, unknown>;
      readonly Origins: Record<string, unknown>[];
    };
    const cachePolicyConfig = cachePolicy.Properties.CachePolicyConfig as Record<string, unknown>;
    const responseHeadersPolicyConfig = responseHeadersPolicy.Properties.ResponseHeadersPolicyConfig as Record<string, unknown>;
    const originId = distributionConfig.Origins[0].Id;
    const selectivePathPatterns = [
      "/api/v1/listing-sources",
      "/api/v1/listing-sources/by-slug/*",
      "/api/v1/auctions",
      "/api/v1/auctions/*",
      "/api/v1/product-listings",
      "/api/v1/product-listings/*",
    ];
    const selectiveBehaviors = distributionConfig.CacheBehaviors.slice(0, selectivePathPatterns.length);
    const broadApiBehavior = distributionConfig.CacheBehaviors[selectivePathPatterns.length];

    expect(domains).toHaveLength(1);
    expect(mappings).toHaveLength(1);
    expect(cachePolicyId).toBeDefined();
    expect(responseHeadersPolicyId).toBeDefined();
    expect(distributionId).toBeDefined();
    expect(cachePolicyConfig).toEqual({
      Comment: `${stage} API selective read cache`,
      DefaultTTL: 0,
      MaxTTL: 900,
      MinTTL: 0,
      Name: `api-${stage}-selective-read-cache`,
      ParametersInCacheKeyAndForwardedToOrigin: {
        CookiesConfig: { CookieBehavior: "none" },
        EnableAcceptEncodingBrotli: true,
        EnableAcceptEncodingGzip: true,
        HeadersConfig: { HeaderBehavior: "whitelist", Headers: ["Authorization", "Origin", "Host"] },
        QueryStringsConfig: { QueryStringBehavior: "all" },
      },
    });
    expect(responseHeadersPolicyConfig).toEqual({
      Comment: `${stage} API selective read response headers`,
      Name: `api-${stage}-selective-read-response-headers`,
      RemoveHeadersConfig: {
        Items: [{ Header: "X-Request-Id" }, { Header: "X-Correlation-Id" }],
      },
    });
    expect(distributionConfig.CacheBehaviors.map((behavior) => behavior.PathPattern)).toEqual([
      ...selectivePathPatterns,
      "/api/*",
    ]);
    for (const [index, behavior] of selectiveBehaviors.entries()) {
      expect(behavior).toEqual(expect.objectContaining({
        AllowedMethods: ["GET", "HEAD", "OPTIONS"],
        CachedMethods: ["GET", "HEAD"],
        CachePolicyId: { Ref: cachePolicyId },
        Compress: true,
        OriginRequestPolicyId: "216adef6-5c7f-47e4-b989-5492eafa07d3",
        PathPattern: selectivePathPatterns[index],
        ResponseHeadersPolicyId: { Ref: responseHeadersPolicyId },
        TargetOriginId: originId,
        ViewerProtocolPolicy: "redirect-to-https",
      }));
      expect(behavior).not.toHaveProperty("LambdaFunctionAssociations");
      expect(behavior).not.toHaveProperty("FunctionAssociations");
    }
    expect(broadApiBehavior).toEqual(expect.objectContaining({
      AllowedMethods: ["GET", "HEAD", "OPTIONS", "PUT", "PATCH", "POST", "DELETE"],
      CachedMethods: ["GET", "HEAD", "OPTIONS"],
      CachePolicyId: "4135ea2d-6df8-44a3-9df3-4b5a84be39ad",
      Compress: true,
      OriginRequestPolicyId: "216adef6-5c7f-47e4-b989-5492eafa07d3",
      PathPattern: "/api/*",
      TargetOriginId: originId,
      ViewerProtocolPolicy: "redirect-to-https",
    }));
    expect(broadApiBehavior).not.toHaveProperty("ResponseHeadersPolicyId");
    expect(distributionConfig.DefaultCacheBehavior).toEqual(expect.objectContaining({
      AllowedMethods: ["GET", "HEAD", "OPTIONS", "PUT", "PATCH", "POST", "DELETE"],
      CachedMethods: ["GET", "HEAD"],
      CachePolicyId: "4135ea2d-6df8-44a3-9df3-4b5a84be39ad",
      Compress: true,
      OriginRequestPolicyId: "216adef6-5c7f-47e4-b989-5492eafa07d3",
      TargetOriginId: originId,
      ViewerProtocolPolicy: "redirect-to-https",
    }));
    expect(distributionConfig.DefaultCacheBehavior).not.toHaveProperty("ResponseHeadersPolicyId");
    expect(distributionConfig.CustomErrorResponses).toEqual(
      [400, 403, 404, 405, 414, 500, 501, 502, 503, 504].map((errorCode) => ({
        ErrorCode: errorCode,
        ErrorCachingMinTTL: 0,
      })),
    );
    expect(distribution.Properties.DistributionConfig).toEqual(expect.objectContaining({ WebACLId: expect.anything() }));
    expect(Object.values(template.findResources("AWS::CloudFront::Function"))).toHaveLength(0);
    expect(Object.values(template.findResources("AWS::ApiGatewayV2::Authorizer"))).toHaveLength(0);
    expect(JSON.stringify(distributionConfig)).not.toMatch(/LambdaFunctionAssociations|FunctionAssociations|Lambda@Edge/);
  });
});

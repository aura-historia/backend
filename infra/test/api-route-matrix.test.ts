import * as cdk from "aws-cdk-lib";
import { Template } from "aws-cdk-lib/assertions";
import * as fs from "node:fs";
import * as path from "node:path";
import { createApplicationStacks } from "../src/application-stack";
import { API_ROUTE_CATALOG, RouteAuthPolicy, type RouteDefinition } from "../src/constructs/api";
import { STAGES, type StageName } from "../src/config";

type CloudFormationResource = {
  readonly Properties: Record<string, unknown>;
};

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
      if (!routePath.startsWith("/api/v1/") && routePath !== "/health" && routePath !== "/ready") {
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

describe("HTTP API route policy matrix", () => {
  test("covers the declared Axum and OpenAPI operations without a catch-all", () => {
    const catalog = catalogRouteKeys();
    const swagger = swaggerRouteKeys();
    const axum = axumRouteKeys();

    expect(catalog).toHaveLength(96);
    expect(new Set(catalog).size).toBe(catalog.length);
    expect(catalog.filter((key) => !key.endsWith(" /health") && !key.endsWith(" /ready"))).toEqual(swagger);
    expect(catalog).toEqual(axum);
    expect(catalog).not.toContain("ANY /{proxy+}");
  });

  test("keeps authentication that accepts non-Cognito credentials in Axum", () => {
    expect(API_ROUTE_CATALOG.filter((route) => route.auth === RouteAuthPolicy.OptionalBearer)).not.toHaveLength(0);
    expect(API_ROUTE_CATALOG.filter((route) => route.auth === RouteAuthPolicy.ApplicationBearer)).not.toHaveLength(0);
    expect(API_ROUTE_CATALOG.filter((route) => route.auth === RouteAuthPolicy.OAuth)).not.toHaveLength(0);
    expect(API_ROUTE_CATALOG).toContainEqual(expect.objectContaining({
      path: "/api/v1/webhooks/woocommerce/{listing_source_id}",
      auth: RouteAuthPolicy.ProviderSignature,
    }));
  });

  test.each(STAGES)("synthesizes the complete %s route matrix to one live API alias", (stage) => {
    const template = apiTemplate(stage);
    const routes = Object.values(template.findResources("AWS::ApiGatewayV2::Route")) as CloudFormationResource[];
    const integrations = Object.values(template.findResources("AWS::ApiGatewayV2::Integration")) as CloudFormationResource[];
    const permissions = Object.values(template.findResources("AWS::Lambda::Permission")) as CloudFormationResource[];

    expect(routes.map((route) => {
      const [method, ...pathParts] = String(route.Properties.RouteKey).split(" ");
      return routeKey(method, pathParts.join(" "));
    }).sort()).toEqual(catalogRouteKeys());
    expect(routes.every((route) => route.Properties.AuthorizationType === "NONE")).toBe(true);
    expect(Object.values(template.findResources("AWS::ApiGatewayV2::Authorizer"))).toHaveLength(0);
    expect(integrations).toHaveLength(1);
    expect(JSON.stringify(integrations[0].Properties.IntegrationUri)).toContain(`aura-historia-api-${stage}:live`);
    expect(permissions).toHaveLength(API_ROUTE_CATALOG.length + (stage === "ephemeral" ? 1 : 0));
    expect(permissions.every((permission) => JSON.stringify(permission.Properties.FunctionName).includes(
      `aura-historia-api-${stage}:live`,
    ))).toBe(true);
  });

  test.each(["dev", "prod"] as const)("retains the %s custom domain, CloudFront and WAF while disabling shared API caching", (stage) => {
    const template = apiTemplate(stage);
    const distributions = Object.values(template.findResources("AWS::CloudFront::Distribution")) as CloudFormationResource[];
    const domains = Object.values(template.findResources("AWS::ApiGatewayV2::DomainName")) as CloudFormationResource[];
    const mappings = Object.values(template.findResources("AWS::ApiGatewayV2::ApiMapping")) as CloudFormationResource[];

    expect(domains).toHaveLength(1);
    expect(mappings).toHaveLength(1);
    expect(distributions).toHaveLength(1);
    expect(Object.values(template.findResources("AWS::CloudFront::Function"))).toHaveLength(0);
    expect(JSON.stringify(distributions[0].Properties)).toContain("4135ea2d-6df8-44a3-9df3-4b5a84be39ad");
    expect(distributions[0].Properties.DistributionConfig).toEqual(expect.objectContaining({ WebACLId: expect.anything() }));
  });
});

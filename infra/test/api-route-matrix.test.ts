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
  type RouteDefinition,
} from "../src/constructs/api";
import { STAGES, stageConfig, type StageName } from "../src/config";

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

    expect(catalog).toHaveLength(96);
    expect(new Set(catalog).size).toBe(catalog.length);
    expect(catalog.filter((key) => !key.endsWith(" /health") && !key.endsWith(" /ready"))).toEqual(swagger);
    expect(catalog).toEqual(axum);
    expect(catalog).not.toContain("ANY /{proxy+}");
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
    const healthRoutes = API_ROUTE_CATALOG.filter((route) => ["/health", "/ready"].includes(route.path));
    expect(healthRoutes.map((route) => routeKey(route.method, route.path)).sort()).toEqual(["GET /health", "GET /ready"]);
    for (const route of healthRoutes) {
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
    for (const path of ["/api/v1/oauth/token", "/api/v1/oauth/revoke", "/api/v1/oauth/introspect"]) {
      expect(API_ROUTE_CATALOG).toContainEqual(expect.objectContaining({
        path,
        policy: expect.objectContaining({ bearer: "NONE", oauthCredentials: OAuthCredentialRequirement.ClientCredentials }),
      }));
    }
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
    expect(routes).toHaveLength(96);
    expect(routes.every((route) => route.Properties.RouteKey !== "$default" && !String(route.Properties.RouteKey).includes("/{proxy+}"))).toBe(true);
    expect(routes.every((route) => route.Properties.AuthorizationType === "NONE")).toBe(true);
    expect(Object.values(template.findResources("AWS::ApiGatewayV2::Authorizer"))).toHaveLength(0);
    expect(integrations).toHaveLength(1);
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
      CacheBehaviors: [expect.objectContaining({ OriginRequestPolicyId: "216adef6-5c7f-47e4-b989-5492eafa07d3" })],
    }));
  });

  test("keeps ephemeral without a custom domain or CloudFront alias", () => {
    const config = stageConfig("ephemeral");
    const template = apiTemplate("ephemeral");
    expect(config.apiDomainName).toBeUndefined();
    expect(config.apiEndpointUrl).toBeUndefined();
    expect(config.apiCloudFrontAliases).toEqual([]);
    expect(config.apiGatewayCertificateArn).toBeUndefined();
    expect(config.apiCloudFrontCertificateArn).toBeUndefined();
    expect(Object.values(template.findResources("AWS::ApiGatewayV2::DomainName"))).toHaveLength(0);
    expect(Object.values(template.findResources("AWS::CloudFront::Distribution"))).toHaveLength(0);
  });

  test("OpenAPI advertises the stage API host without changing production", () => {
    const swagger = fs.readFileSync(path.join(__dirname, "../../docs/swagger.yaml"), "utf8");
    expect(swagger).toContain("url: https://api.stage.aura-historia.com");
    expect(swagger).toContain("url: https://api.aura-historia.com");
    expect(swagger).not.toContain("api.dev.aura-historia.com");
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

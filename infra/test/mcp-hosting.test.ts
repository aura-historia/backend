import * as cdk from "aws-cdk-lib";
import { Template } from "aws-cdk-lib/assertions";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { createApplicationStacks } from "../src/application-stack";
import { STAGES, stageConfig } from "../src/config";
import { MCP_CORS_ALLOW_HEADERS, MCP_CORS_EXPOSE_HEADERS, MCP_ROUTE_CATALOG } from "../src/constructs/mcp";
import { MCP_MAX_REQUEST_BODY_BYTES, MCP_OVERSIZED_CONTENT_LENGTH_PATTERN } from "../src/constructs/mcp-web-acl";
import { templateForConstruct } from "./construct-template";

describe.each(STAGES)("%s MCP hosting", (stage) => {
  const config = stageConfig(stage);
  let mcp: Template;
  let compute: Template;
  let fullApi: Template;

  beforeAll(() => {
    const app = new cdk.App({ analyticsReporting: false });
    const stacks = createApplicationStacks(app, { stage, env: { account: "123456789012", region: "eu-central-1" } });
    compute = Template.fromStack(stacks.compute);
    mcp = templateForConstruct(stacks.api, stacks.api.mcp);
    fullApi = Template.fromStack(stacks.api);
  });

  test("exposes only the protocol, probes and exact reserved metadata paths through one MCP live alias", () => {
    const routes = Object.values(mcp.findResources("AWS::ApiGatewayV2::Route"));
    for (const type of ["AWS::ApiGatewayV2::Api", "AWS::ApiGatewayV2::Integration", "AWS::Lambda::Permission"]) {
      expect(Object.keys(mcp.findResources(type))).toHaveLength(1);
    }
    const [[apiId, api]] = Object.entries(mcp.findResources("AWS::ApiGatewayV2::Api"));
    const [[integrationId, integration]] = Object.entries(mcp.findResources("AWS::ApiGatewayV2::Integration"));
    const [permission] = Object.values(mcp.findResources("AWS::Lambda::Permission"));
    expect(routes.map((route) => route.Properties.RouteKey).sort()).toEqual([
      "ANY /v1", "GET /health", "HEAD /health", "GET /ready", "HEAD /ready",
      "GET /.well-known/oauth-protected-resource", "HEAD /.well-known/oauth-protected-resource",
      "GET /.well-known/oauth-protected-resource/v1", "HEAD /.well-known/oauth-protected-resource/v1",
    ].sort());
    expect(MCP_ROUTE_CATALOG.map((route) => `${route.method} ${route.path}`).sort())
      .toEqual(routes.map((route) => route.Properties.RouteKey).sort());
    for (const route of routes) {
      expect(route.Properties.ApiId).toEqual({ Ref: apiId });
      expect(route.Properties.AuthorizationType).toBe("NONE");
      expect(JSON.stringify(route.Properties.Target)).toContain(integrationId);
      expect(route.Properties.RouteKey).not.toMatch(/\*|\{|\$default/);
    }
    expect(api.Properties.DisableExecuteApiEndpoint).toBe(true);
    expect(integration.Properties).toMatchObject({ IntegrationType: "AWS_PROXY", PayloadFormatVersion: "2.0", TimeoutInMillis: 29000 });
    expect(integration.Properties).not.toHaveProperty("RequestParameters");
    expect(integration.Properties).not.toHaveProperty("ResponseParameters");
    expect(integration.Properties.IntegrationUri).toEqual({ "Fn::Join": ["", [
      "arn:", { Ref: "AWS::Partition" }, `:lambda:eu-central-1:123456789012:function:aura-historia-mcp-${stage}:live`,
    ]] });
    expect(permission.Properties).toMatchObject({
      Action: "lambda:InvokeFunction", Principal: "apigateway.amazonaws.com",
      FunctionName: integration.Properties.IntegrationUri,
    });
    expect(JSON.stringify(permission.Properties.SourceArn)).toContain(apiId);
    expect(mcp.findResources("AWS::ApiGatewayV2::Authorizer")).toEqual({});
    // Keep MCP's ANY admission wholly outside the closed REST catalog.
    const allRoutes = Object.values(fullApi.findResources("AWS::ApiGatewayV2::Route"));
    expect(allRoutes.filter((route) => route.Properties.RouteKey === "ANY /v1")).toHaveLength(1);
    expect(allRoutes.some((route) => /\{proxy\+\}|\$default/.test(route.Properties.RouteKey))).toBe(false);
  });

  test("uses exact browser origins and carries the full protocol and challenge header contract", () => {
    const [api] = Object.values(mcp.findResources("AWS::ApiGatewayV2::Api"));
    expect(api.Properties.CorsConfiguration).toEqual({
      AllowHeaders: MCP_CORS_ALLOW_HEADERS,
      AllowMethods: ["GET", "HEAD", "POST", "OPTIONS"],
      AllowOrigins: config.mcp.allowedOrigins,
      ExposeHeaders: MCP_CORS_EXPOSE_HEADERS,
      MaxAge: 0,
    });
    expect(MCP_CORS_ALLOW_HEADERS).toEqual([
      "Authorization", "Content-Type", "Accept", "MCP-Protocol-Version", "Mcp-Method", "Mcp-Name",
      "Mcp-Session-Id", "X-Correlation-Id",
    ]);
    expect(MCP_CORS_EXPOSE_HEADERS).toEqual(["WWW-Authenticate", "X-Request-Id", "X-Correlation-Id", "Content-Type"]);
    expect(config.mcp.allowedOrigins).toEqual(stage === "prod"
      ? ["https://aura-historia.com"] : ["https://stage.aura-historia.com", "http://localhost:3000"]);
    expect(config.mcp.allowedOrigins.every((origin) => !origin.includes("*") && origin !== "null")).toBe(true);
  });

  test("keeps the public /v1 path and Host intact through exact certificates and regional origin mapping", () => {
    const [[domainId, domain]] = Object.entries(mcp.findResources("AWS::ApiGatewayV2::DomainName"));
    const [[apiId]] = Object.entries(mcp.findResources("AWS::ApiGatewayV2::Api"));
    const [mapping] = Object.values(mcp.findResources("AWS::ApiGatewayV2::ApiMapping"));
    const [distribution] = Object.values(mcp.findResources("AWS::CloudFront::Distribution"));
    const host = stage === "prod" ? "mcp.aura-historia.com" : "mcp.stage.aura-historia.com";
    expect(config.mcp.domainName).toBe(host);
    expect(domain.Properties).toMatchObject({
      DomainName: host,
      DomainNameConfigurations: [{
        CertificateArn: `{{resolve:ssm:/certificates/${stage}/mcp-regional-certificate-arn}}`,
        EndpointType: "REGIONAL", SecurityPolicy: "TLS_1_2",
      }],
      RoutingMode: "API_MAPPING_ONLY",
    });
    expect(mapping.Properties).toEqual({ ApiId: { Ref: apiId }, DomainName: { Ref: domainId }, Stage: stage });
    const front = distribution.Properties.DistributionConfig;
    expect(front.Aliases).toEqual([host]);
    expect(front.ViewerCertificate.AcmCertificateArn).toBe(`{{resolve:ssm:/certificates/${stage}/mcp-cloudfront-certificate-arn}}`);
    expect(front.Origins).toEqual([{
      Id: "McpHttpApiOrigin", DomainName: { "Fn::GetAtt": [domainId, "RegionalDomainName"] },
      CustomOriginConfig: {
        HTTPPort: 80, HTTPSPort: 443, OriginKeepaliveTimeout: 5, OriginProtocolPolicy: "https-only",
        OriginReadTimeout: 30, OriginSSLProtocols: ["TLSv1.2"],
      },
    }]);
    expect(front.DefaultCacheBehavior.OriginRequestPolicyId).toBe("216adef6-5c7f-47e4-b989-5492eafa07d3");
    expect(mcp.toJSON().Outputs.McpEndpointUrl.Value).toBe(`https://${host}/v1`);
    expect(JSON.stringify(front)).not.toMatch(/OriginPath|LambdaFunctionAssociations|FunctionAssociations/);
    expect(mcp.findResources("AWS::Route53::RecordSet")).toEqual({});
  });

  test("disables every shared cache and adds no-store to preflight and Gateway errors without stripping challenges or trace IDs", () => {
    const [[policyId, responseHeaders]] = Object.entries(mcp.findResources("AWS::CloudFront::ResponseHeadersPolicy"));
    const [distribution] = Object.values(mcp.findResources("AWS::CloudFront::Distribution"));
    const front = distribution.Properties.DistributionConfig;
    expect(front.DefaultCacheBehavior).toMatchObject({
      AllowedMethods: ["GET", "HEAD", "OPTIONS", "PUT", "PATCH", "POST", "DELETE"],
      CachedMethods: ["GET", "HEAD"], CachePolicyId: "4135ea2d-6df8-44a3-9df3-4b5a84be39ad",
      ResponseHeadersPolicyId: { Ref: policyId }, TargetOriginId: "McpHttpApiOrigin",
    });
    expect(front.CacheBehaviors).toBeUndefined();
    expect(responseHeaders.Properties.ResponseHeadersPolicyConfig).toEqual({
      Name: `mcp-${stage}-no-store`,
      CustomHeadersConfig: { Items: [{ Header: "Cache-Control", Value: "no-store", Override: true }] },
    });
    expect(front.CustomErrorResponses).toEqual([400, 403, 404, 405, 414, 416, 500, 501, 502, 503, 504]
      .map((ErrorCode) => ({ ErrorCode, ErrorCachingMinTTL: 0 })));
    expect(mcp.findResources("AWS::CloudFront::CachePolicy")).toEqual({});
    expect(JSON.stringify(front)).not.toContain("aura-historia-api");
  });

  test("allocates a bounded public Lambda with listing providers and only runtime database IAM", () => {
    const [[functionId, fn]] = Object.entries(compute.findResources("AWS::Lambda::Function"))
      .filter(([, resource]) => resource.Properties.FunctionName === `aura-historia-mcp-${stage}`);
    const environment = fn.Properties.Environment.Variables;
    expect(fn.Properties).toMatchObject({
      Runtime: "provided.al2023", Handler: "lib.handler", Architectures: ["x86_64"],
      MemorySize: 512, Timeout: 28, ReservedConcurrentExecutions: stage === "prod" ? 5 : 2,
    });
    expect(fn.Properties.VpcConfig).toBeDefined();
    expect(fn.Properties.Layers).toHaveLength(1);
    expect(environment).toMatchObject({
      AWS_LAMBDA_HTTP_IGNORE_STAGE_IN_PATH: "true", COMMIT_SHA: { Ref: "CommitSHA" },
      MCP_ALLOWED_HOSTS: config.mcp.domainName, MCP_ALLOWED_ORIGINS: config.mcp.allowedOrigins.join(","),
      STAGE: stage,
      OPENSEARCH_ENDPOINT_URL: config.opensearchEndpointUrl,
      OPENSEARCH_USERNAME: `{{resolve:ssm:/opensearch/${stage}/reader/username}}`,
      OPENSEARCH_PASSWORD: `{{resolve:ssm:/opensearch/${stage}/reader/password}}`,
      VERTEX_AI_LOCATION: `{{resolve:ssm:/vertex-ai/${stage}/location}}`,
      VERTEX_AI_PROJECT_ID: `{{resolve:ssm:/vertex-ai/${stage}/project-id}}`,
      AURA_HISTORIA_GOOGLE_ADC_CREDENTIALS_JSON: `{{resolve:ssm:/secrets/${stage}/google-application-credentials}}`,
      POSTGRES_DATABASE: "aura_historia", POSTGRES_MAX_CONNECTIONS: "1",
      POSTGRES_TLS_ROOT_CERT: "/opt/aura-historia/rds-ca/global-bundle.pem",
    });
    expect(Object.keys(environment).sort()).toEqual([
      "AWS_LAMBDA_HTTP_IGNORE_STAGE_IN_PATH", "COMMIT_SHA", "MCP_ALLOWED_HOSTS", "MCP_ALLOWED_ORIGINS",
      "STAGE", "OPENSEARCH_ENDPOINT_URL", "OPENSEARCH_USERNAME", "OPENSEARCH_PASSWORD",
      "VERTEX_AI_LOCATION", "VERTEX_AI_PROJECT_ID", "AURA_HISTORIA_GOOGLE_ADC_CREDENTIALS_JSON",
      "POSTGRES_DATABASE", "POSTGRES_HOST", "POSTGRES_MAX_CONNECTIONS", "POSTGRES_PORT", "POSTGRES_SECRET_ARN", "POSTGRES_TLS_ROOT_CERT",
    ].sort());
    const budget = fn.Properties.ReservedConcurrentExecutions * Number(environment.POSTGRES_MAX_CONNECTIONS);
    expect(budget).toBe(stage === "prod" ? 5 : 2);
    const rest = Object.values(compute.findResources("AWS::Lambda::Function"))
      .find((resource) => resource.Properties.FunctionName === `aura-historia-api-${stage}`)!;
    expect(rest.Properties.ReservedConcurrentExecutions).toBeUndefined();
    expect(rest.Properties.Environment.Variables.POSTGRES_MAX_CONNECTIONS).toBe("1");
    for (const key of ["STAGE", "OPENSEARCH_ENDPOINT_URL", "OPENSEARCH_USERNAME", "OPENSEARCH_PASSWORD",
      "VERTEX_AI_LOCATION", "VERTEX_AI_PROJECT_ID", "AURA_HISTORIA_GOOGLE_ADC_CREDENTIALS_JSON"]) {
      expect(environment[key]).toEqual(rest.Properties.Environment.Variables[key]);
    }
    const roleId = fn.Properties.Role["Fn::GetAtt"][0];
    const statements = Object.values(compute.findResources("AWS::IAM::Policy"))
      .filter((policy) => policy.Properties.Roles.some((role: { Ref: string }) => role.Ref === roleId))
      .flatMap((policy) => policy.Properties.PolicyDocument.Statement)
      .filter((statement) => !JSON.stringify(statement.Action).includes("logs:"));
    expect(statements).toEqual([{ Action: "secretsmanager:GetSecretValue", Effect: "Allow", Resource: environment.POSTGRES_SECRET_ARN }]);
    const [alias] = Object.values(compute.findResources("AWS::Lambda::Alias"))
      .filter((resource) => resource.Properties.FunctionName.Ref === functionId);
    expect(alias.Properties.Name).toBe("live");
    const versionId = alias.Properties.FunctionVersion["Fn::GetAtt"][0];
    const version = compute.findResources("AWS::Lambda::Version")[versionId];
    expect(version.Properties.FunctionName).toEqual({ Ref: functionId });
    expect(version.Properties.Description).toEqual({ "Fn::Join": ["", ["aura-historia-mcp-", { Ref: "CommitSHA" }]] });
    expect(fn.Properties.Code.S3Key).toEqual({ "Fn::Join": ["", [`aura-historia-mcp-${stage}-`, { Ref: "CommitSHA" }, ".zip"]] });
    expect(compute.findResources("AWS::Lambda::Url")).toEqual({});
  });

  test("admits bounded JSON-RPC bodies only on POST /v1 while retaining managed protections and disabling request sampling", () => {
    const [acl] = Object.values(mcp.findResources("AWS::CloudFormation::CustomResource"));
    const rules = acl.Properties.Rules;
    expect(acl.Properties.SampledRequestsEnabled).toBe(false);
    expect(rules.map((rule: { Name: string }) => rule.Name)).toEqual([
      "MCP-RequestBodyLimit", "AWS-AWSManagedRulesAmazonIpReputationList", "AWS-AWSManagedRulesCommonRuleSet",
      "MCP-BodySizeOutsideProtocol", "AWS-AWSManagedRulesKnownBadInputsRuleSet",
    ]);
    expect(rules.map((rule: { Priority: number }) => rule.Priority)).toEqual([0, 1, 2, 3, 4]);
    expect(rules.every((rule: { VisibilityConfig: { SampledRequestsEnabled: boolean } }) => !rule.VisibilityConfig.SampledRequestsEnabled)).toBe(true);
    expect(rules[0].Statement.RegexMatchStatement).toEqual({
      FieldToMatch: { SingleHeader: { Name: "content-length" } }, RegexString: MCP_OVERSIZED_CONTENT_LENGTH_PATTERN,
      TextTransformations: [{ Priority: 0, Type: "NONE" }],
    });
    expect(rules[0].Action.Block.CustomResponse).toEqual({
      CustomResponseBodyKey: "payload-too-large", ResponseCode: 413,
      ResponseHeaders: [{ Name: "Cache-Control", Value: "no-store" }],
    });
    expect(JSON.parse(acl.Properties.CustomResponseBodies["payload-too-large"].Content)).toEqual({ error: "request_too_large" });
    expect(rules[2].Statement.ManagedRuleGroupStatement.RuleActionOverrides).toEqual([
      { Name: "NoUserAgent_HEADER", ActionToUse: { Count: {} } },
      { Name: "SizeRestrictions_BODY", ActionToUse: { Count: {} } },
    ]);
    expect(rules[1].Statement.ManagedRuleGroupStatement.RuleActionOverrides).toBeUndefined();
    expect(rules[4].Statement.ManagedRuleGroupStatement.RuleActionOverrides).toBeUndefined();
    expect(rules[3].Statement).toEqual({ AndStatement: { Statements: [
      { LabelMatchStatement: { Key: "awswaf:managed:aws:core-rule-set:SizeRestrictions_Body", Scope: "LABEL" } },
      { NotStatement: { Statement: { AndStatement: { Statements: [
        { RegexMatchStatement: { FieldToMatch: { UriPath: {} }, RegexString: "^/v1$", TextTransformations: [{ Priority: 0, Type: "NONE" }] } },
        { RegexMatchStatement: { FieldToMatch: { Method: {} }, RegexString: "^POST$", TextTransformations: [{ Priority: 0, Type: "NONE" }] } },
      ] } } } },
    ] } });
    for (const rule of [rules[1], rules[2], rules[4]]) expect(rule.OverrideAction).toEqual({ None: {} });
    expect(mcp.findResources("AWS::WAFv2::LoggingConfiguration")).toEqual({});
  });

  test("records bounded operational fields without payload, Authorization, Origin or query logging", () => {
    const [gatewayStage] = Object.values(mcp.findResources("AWS::ApiGatewayV2::Stage"));
    expect(JSON.parse(gatewayStage.Properties.AccessLogSettings.Format)).toEqual({
      requestId: "$context.requestId", routeKey: "$context.routeKey", status: "$context.status",
      integrationLatency: "$context.integrationLatency",
    });
    const logs = Object.values(compute.findResources("AWS::Logs::LogGroup"));
    expect(logs.find((log) => log.Properties.LogGroupName === `/aws/lambda/aura-historia-mcp-${stage}`)?.Properties.RetentionInDays).toBe(30);
    expect(gatewayStage.Properties.DefaultRouteSettings).toMatchObject({
      ThrottlingBurstLimit: stage === "prod" ? 10 : 4, ThrottlingRateLimit: stage === "prod" ? 20 : 5,
    });
  });
});

test("keeps front-door payload and timeout budgets inside the actual buffered transport and Lambda limits", () => {
  const transport = readFileSync(resolve(__dirname, "../../src/aura-historia-mcp/src/config.rs"), "utf8");
  const body = transport.match(/const MAX_REQUEST_BYTES: usize = ([0-9_* ]+);/);
  expect(body).not.toBeNull();
  expect(body![1].split("*").map((part) => Number(part.trim().replaceAll("_", ""))).reduce((a, b) => a * b))
    .toBe(MCP_MAX_REQUEST_BODY_BYTES);
  expect(transport).toMatch(/MAX_RESPONSE_BYTES: usize = 2 \* 1024 \* 1024/);
  expect(transport).toMatch(/REQUEST_TIMEOUT: Duration = Duration::from_secs\(25\)/);
  expect(MCP_MAX_REQUEST_BODY_BYTES).toBe(262144);
});

test("recognizes precisely declared lengths above the transport's request budget", () => {
  const oversized = new RegExp(MCP_OVERSIZED_CONTENT_LENGTH_PATTERN);
  for (let length = 0; length <= 1_100_000; length += 1) {
    if (oversized.test(String(length)) !== (length > MCP_MAX_REQUEST_BODY_BYTES)) {
      throw new Error(`Content-Length ${length} classified incorrectly`);
    }
  }
  expect(oversized.test("000262144")).toBe(false);
  expect(oversized.test("000262145")).toBe(true);
  expect(oversized.test("999999999999999999999999999999")).toBe(true);
});

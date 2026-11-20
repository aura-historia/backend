import * as cdk from "aws-cdk-lib";
import type { Template } from "aws-cdk-lib/assertions";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { createApplicationStacks } from "../src/application-stack";
import { STAGES, stageConfig } from "../src/config";
import { MCP_SERVER_CARD_PATH } from "../src/constructs/mcp";
import { MCP_MAX_REQUEST_BODY_BYTES, MCP_OVERSIZED_CONTENT_LENGTH_PATTERN } from "../src/constructs/mcp-web-acl";
import { templateForConstruct } from "./construct-template";
import { expectHostOrigin } from "./host-origin-assertions";

describe.each(STAGES)("%s MCP hosting", (stage) => {
  const config = stageConfig(stage);
  let mcp: Template;

  beforeAll(() => {
    const app = new cdk.App({ analyticsReporting: false });
    const stacks = createApplicationStacks(app, { stage, env: { account: "123456789012", region: "eu-central-1" } });
    mcp = templateForConstruct(stacks.api, stacks.api.mcp);
  });

  test("keeps the public /v1 path and Host intact from the exact public host to the locked host origin", () => {
    const [distribution] = Object.values(mcp.findResources("AWS::CloudFront::Distribution"));
    const host = "mcp.aura-historia.com";
    expect(config.mcp.domainName).toBe(host);
    const front = distribution.Properties.DistributionConfig;
    expect(front.Aliases).toEqual([host]);
    expect(front.ViewerCertificate.AcmCertificateArn).toBe(`{{resolve:ssm:/certificates/${stage}/mcp-cloudfront-certificate-arn}}`);
    expect(front.Origins).toHaveLength(1);
    expectHostOrigin(front.Origins[0], stage, "McpHttpApiOrigin");
    expect(front.DefaultCacheBehavior.OriginRequestPolicyId).toBe("216adef6-5c7f-47e4-b989-5492eafa07d3");
    expect(mcp.toJSON().Outputs.McpEndpointUrl.Value).toBe(`https://${host}/v1`);
    expect(JSON.stringify(front)).not.toMatch(/OriginPath|LambdaFunctionAssociations|FunctionAssociations/);
    expect(mcp.findResources("AWS::Route53::RecordSet")).toEqual({});
  });

  test("disables shared caching on every protocol behavior and adds no-store to preflight and proxy errors without stripping challenges or trace IDs", () => {
    const policies = mcp.findResources("AWS::CloudFront::ResponseHeadersPolicy");
    const [[policyId, responseHeaders]] = Object.entries(policies)
      .filter(([, policy]) => policy.Properties.ResponseHeadersPolicyConfig.Name === `mcp-${stage}-no-store`);
    const [distribution] = Object.values(mcp.findResources("AWS::CloudFront::Distribution"));
    const front = distribution.Properties.DistributionConfig;
    expect(front.DefaultCacheBehavior).toMatchObject({
      AllowedMethods: ["GET", "HEAD", "OPTIONS", "PUT", "PATCH", "POST", "DELETE"],
      CachedMethods: ["GET", "HEAD"], CachePolicyId: "4135ea2d-6df8-44a3-9df3-4b5a84be39ad",
      ResponseHeadersPolicyId: { Ref: policyId }, TargetOriginId: "McpHttpApiOrigin",
    });
    expect(front.CacheBehaviors.map((behavior: { PathPattern: string }) => behavior.PathPattern))
      .toEqual(["/.well-known/mcp/server-card.json"]);
    expect(Object.keys(policies)).toHaveLength(2);
    expect(responseHeaders.Properties.ResponseHeadersPolicyConfig).toEqual({
      Name: `mcp-${stage}-no-store`,
      CustomHeadersConfig: { Items: [{ Header: "Cache-Control", Value: "no-store", Override: true }] },
    });
    expect(front.CustomErrorResponses).toEqual([400, 403, 404, 405, 414, 416, 500, 501, 502, 503, 504]
      .map((ErrorCode) => ({ ErrorCode, ErrorCachingMinTTL: 0 })));
    expect(JSON.stringify(front)).not.toContain("aura-historia-api");
  });

  test("routes the server card at the path the MCP transport serves and the discovery documents link", () => {
    const declaration = (file: string, name: string) => readFileSync(resolve(__dirname, "../..", file), "utf8")
      .match(new RegExp(`const ${name}: &str = "([^"]+)";`))?.[1];
    expect(declaration("src/aura-historia-mcp/src/server_card.rs", "SERVER_CARD_PATH")).toBe(MCP_SERVER_CARD_PATH);
    expect(declaration("src/agent-discovery/src/environment.rs", "MCP_SERVER_CARD_PATH"))
      .toBe(MCP_SERVER_CARD_PATH);
  });

  test("lets any origin read the server card and caches only it, briefly and without credentials in the key", () => {
    const [distribution] = Object.values(mcp.findResources("AWS::CloudFront::Distribution"));
    const [[cachePolicyId, cachePolicy]] = Object.entries(mcp.findResources("AWS::CloudFront::CachePolicy"));
    const [[cardHeadersId, cardHeaders]] = Object.entries(mcp.findResources("AWS::CloudFront::ResponseHeadersPolicy"))
      .filter(([, policy]) => policy.Properties.ResponseHeadersPolicyConfig.Name === `mcp-${stage}-server-card`);
    const [card] = distribution.Properties.DistributionConfig.CacheBehaviors;
    expect(card).toEqual({
      PathPattern: "/.well-known/mcp/server-card.json",
      AllowedMethods: ["GET", "HEAD", "OPTIONS"], CachedMethods: ["GET", "HEAD"],
      CachePolicyId: { Ref: cachePolicyId }, Compress: true,
      OriginRequestPolicyId: "216adef6-5c7f-47e4-b989-5492eafa07d3",
      ResponseHeadersPolicyId: { Ref: cardHeadersId }, TargetOriginId: "McpHttpApiOrigin",
      ViewerProtocolPolicy: "redirect-to-https",
    });
    expect(cachePolicy.Properties.CachePolicyConfig).toEqual({
      Name: `mcp-${stage}-server-card`, Comment: `${stage} MCP server card`,
      MinTTL: 0, DefaultTTL: 0, MaxTTL: 300,
      ParametersInCacheKeyAndForwardedToOrigin: {
        CookiesConfig: { CookieBehavior: "none" },
        EnableAcceptEncodingBrotli: true, EnableAcceptEncodingGzip: true,
        HeadersConfig: { HeaderBehavior: "none" },
        QueryStringsConfig: { QueryStringBehavior: "none" },
      },
    });
    expect(cardHeaders.Properties.ResponseHeadersPolicyConfig).toEqual({
      Name: `mcp-${stage}-server-card`,
      // The origin's own Cache-Control wins; only proxy-generated errors lack one.
      CustomHeadersConfig: { Items: [{ Header: "Cache-Control", Value: "no-store", Override: false }] },
      CorsConfig: {
        AccessControlAllowCredentials: false,
        AccessControlAllowHeaders: { Items: ["*"] },
        AccessControlAllowMethods: { Items: ["GET", "HEAD", "OPTIONS"] },
        AccessControlAllowOrigins: { Items: ["*"] },
        OriginOverride: true,
      },
    });
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
});

test("keeps front-door payload and timeout budgets consistent with the buffered transport", () => {
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

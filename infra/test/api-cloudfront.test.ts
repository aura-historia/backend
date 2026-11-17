import * as cdk from "aws-cdk-lib";
import type { Template } from "aws-cdk-lib/assertions";
import { createApplicationStacks } from "../src/application-stack";
import { API_SELECTIVE_READ_PATH_PATTERNS } from "../src/constructs/api";
import { STAGES, stageConfig, type StageName } from "../src/config";
import { templateForConstruct } from "./construct-template";
import { expectHostOrigin } from "./host-origin-assertions";

type CloudFormationResource = {
  readonly Properties: Record<string, any>;
};

const ALL_VIEWER = "216adef6-5c7f-47e4-b989-5492eafa07d3";
const CACHING_DISABLED = "4135ea2d-6df8-44a3-9df3-4b5a84be39ad";

function apiTemplate(stage: StageName): Template {
  const stacks = createApplicationStacks(new cdk.App({ analyticsReporting: false }), { stage });
  return templateForConstruct(stacks.api, stacks.api.api);
}

function only(template: Template, type: string): [string, CloudFormationResource] {
  const entries = Object.entries(template.findResources(type)) as [string, CloudFormationResource][];
  expect(entries).toHaveLength(1);
  return entries[0];
}

describe.each(STAGES)("%s REST CloudFront distribution", (stage) => {
  const config = stageConfig(stage);
  let template: Template;
  beforeAll(() => { template = apiTemplate(stage); });

  test("serves the exact public API host from the host origin, locked by the secret header", () => {
    const [, distribution] = only(template, "AWS::CloudFront::Distribution");
    const front = distribution.Properties.DistributionConfig;
    expect(config.apiDomainName).toBe("api.aura-historia.com");
    expect(front.Aliases).toEqual(["api.aura-historia.com"]);
    expect(front.ViewerCertificate).toEqual({
      AcmCertificateArn: `{{resolve:ssm:/certificates/${stage}/api-cloudfront-certificate-arn}}`,
      MinimumProtocolVersion: "TLSv1.2_2021",
      SslSupportMethod: "sni-only",
    });
    expect(front.Origins).toHaveLength(1);
    expectHostOrigin(front.Origins[0], stage, "HttpApiOrigin");
    // AllViewer forwards the public Host, which the host proxy routes and serves certificates by.
    expect(front.DefaultCacheBehavior.OriginRequestPolicyId).toBe(ALL_VIEWER);
    expect(JSON.stringify(front)).not.toMatch(/OriginPath|LambdaFunctionAssociations|FunctionAssociations/);
  });

  test("keeps selective anonymous caching and everything else uncached", () => {
    const [cachePolicyId, cachePolicy] = only(template, "AWS::CloudFront::CachePolicy");
    const [responseHeadersPolicyId, responseHeadersPolicy] = only(template, "AWS::CloudFront::ResponseHeadersPolicy");
    const [, distribution] = only(template, "AWS::CloudFront::Distribution");
    const front = distribution.Properties.DistributionConfig;

    expect(cachePolicy.Properties.CachePolicyConfig).toEqual({
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
    expect(responseHeadersPolicy.Properties.ResponseHeadersPolicyConfig).toEqual({
      Comment: `${stage} API selective read response headers`,
      Name: `api-${stage}-selective-read-response-headers`,
      RemoveHeadersConfig: { Items: [{ Header: "X-Request-Id" }, { Header: "X-Correlation-Id" }] },
    });
    expect(front.CacheBehaviors.map((behavior: { PathPattern: string }) => behavior.PathPattern))
      .toEqual([...API_SELECTIVE_READ_PATH_PATTERNS, "/v1/*"]);
    for (const behavior of front.CacheBehaviors.slice(0, API_SELECTIVE_READ_PATH_PATTERNS.length)) {
      expect(behavior).toEqual({
        AllowedMethods: ["GET", "HEAD", "OPTIONS"],
        CachedMethods: ["GET", "HEAD"],
        CachePolicyId: { Ref: cachePolicyId },
        Compress: true,
        OriginRequestPolicyId: ALL_VIEWER,
        PathPattern: behavior.PathPattern,
        ResponseHeadersPolicyId: { Ref: responseHeadersPolicyId },
        TargetOriginId: "HttpApiOrigin",
        ViewerProtocolPolicy: "redirect-to-https",
      });
    }
    const broad = front.CacheBehaviors[API_SELECTIVE_READ_PATH_PATTERNS.length];
    expect(broad).toEqual({
      AllowedMethods: ["GET", "HEAD", "OPTIONS", "PUT", "PATCH", "POST", "DELETE"],
      CachedMethods: ["GET", "HEAD", "OPTIONS"],
      CachePolicyId: CACHING_DISABLED,
      Compress: true,
      OriginRequestPolicyId: ALL_VIEWER,
      PathPattern: "/v1/*",
      TargetOriginId: "HttpApiOrigin",
      ViewerProtocolPolicy: "redirect-to-https",
    });
    expect(front.DefaultCacheBehavior).toEqual({
      AllowedMethods: ["GET", "HEAD", "OPTIONS", "PUT", "PATCH", "POST", "DELETE"],
      CachedMethods: ["GET", "HEAD"],
      CachePolicyId: CACHING_DISABLED,
      Compress: true,
      OriginRequestPolicyId: ALL_VIEWER,
      TargetOriginId: "HttpApiOrigin",
      ViewerProtocolPolicy: "redirect-to-https",
    });
    expect(front.CustomErrorResponses).toEqual([400, 403, 404, 405, 414, 500, 501, 502, 503, 504]
      .map((ErrorCode) => ({ ErrorCode, ErrorCachingMinTTL: 0 })));
    expect(front.WebACLId).toEqual(expect.anything());
  });

  test("keeps the deployed logical IDs of the distribution, its policies and its Web ACL", () => {
    expect(Object.keys(template.toJSON().Resources).sort()).toEqual([
      "HttpApiApiDistribution148F2DC6",
      "HttpApiApiWebAclCustomResourceFunctionEF509EBC",
      "HttpApiApiWebAclCustomResourceFunctionServiceRole2450465F",
      "HttpApiApiWebAclCustomResourceFunctionServiceRoleDefaultPolicy2B22D6C7",
      "HttpApiApiWebAclFB6103FB",
      "HttpApiSelectiveReadCachePolicy8FFACE52",
      "HttpApiSelectiveReadResponseHeadersPolicy8D396999",
    ]);
  });
});

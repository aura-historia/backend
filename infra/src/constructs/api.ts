import * as cloudfront from "aws-cdk-lib/aws-cloudfront";
import { Construct } from "constructs";
import type { StageConfig } from "../config";
import { createCloudFrontWebAcl } from "./cloudfront-web-acl";
import { hostOrigin } from "./host-origin";

const CLOUDFRONT_CACHING_DISABLED_POLICY_ID = "4135ea2d-6df8-44a3-9df3-4b5a84be39ad";
const CLOUDFRONT_ALL_VIEWER_ORIGIN_REQUEST_POLICY_ID = "216adef6-5c7f-47e4-b989-5492eafa07d3";

/** Path patterns whose approved anonymous representations CloudFront may share. */
export const API_SELECTIVE_READ_PATH_PATTERNS = [
  "/v1/listing-sources",
  "/v1/listing-sources/by-slug/*",
  "/v1/auctions",
  "/v1/auctions/*",
  "/v1/product-listings",
  "/v1/product-listings/*",
  "/v1/overview",
  "/.well-known/api-catalog",
  "/v1/site-discovery/*",
] as const;

export interface ApiCloudFrontProps {
  readonly config: StageConfig;
}

/**
 * The REST API's public CloudFront distribution, its cache and response policies and its Web
 * ACL, in front of the host origin. Axum owns routing, authentication and CORS.
 */
export class ApiCloudFront extends Construct {
  readonly distribution: cloudfront.CfnDistribution;

  constructor(scope: Construct, id: string, props: ApiCloudFrontProps) {
    super(scope, id);

    const originId = "HttpApiOrigin";
    const cachePolicy = new cloudfront.CfnCachePolicy(this, "SelectiveReadCachePolicy", {
      cachePolicyConfig: {
        comment: `${props.config.stage} API selective read cache`,
        defaultTtl: 0,
        maxTtl: 900,
        minTtl: 0,
        name: `api-${props.config.stage}-selective-read-cache`,
        parametersInCacheKeyAndForwardedToOrigin: {
          cookiesConfig: { cookieBehavior: "none" },
          enableAcceptEncodingBrotli: true,
          enableAcceptEncodingGzip: true,
          headersConfig: {
            headerBehavior: "whitelist",
            headers: ["Authorization", "Origin", "Host"],
          },
          queryStringsConfig: { queryStringBehavior: "all" },
        },
      },
    });
    const responseHeadersPolicy = new cloudfront.CfnResponseHeadersPolicy(this, "SelectiveReadResponseHeadersPolicy", {
      responseHeadersPolicyConfig: {
        comment: `${props.config.stage} API selective read response headers`,
        name: `api-${props.config.stage}-selective-read-response-headers`,
        removeHeadersConfig: {
          items: [{ header: "X-Request-Id" }, { header: "X-Correlation-Id" }],
        },
      },
    });
    const webAclArn = this.configureCloudFrontWebAcl(props);

    this.distribution = new cloudfront.CfnDistribution(this, "ApiDistribution", {
      distributionConfig: {
        aliases: props.config.apiCloudFrontAliases,
        cacheBehaviors: [
          ...API_SELECTIVE_READ_PATH_PATTERNS.map((pathPattern) => ({
            allowedMethods: ["GET", "HEAD", "OPTIONS"],
            cachedMethods: ["GET", "HEAD"],
            cachePolicyId: cachePolicy.ref,
            compress: true,
            originRequestPolicyId: CLOUDFRONT_ALL_VIEWER_ORIGIN_REQUEST_POLICY_ID,
            pathPattern,
            responseHeadersPolicyId: responseHeadersPolicy.ref,
            targetOriginId: originId,
            viewerProtocolPolicy: "redirect-to-https",
          })),
          {
            allowedMethods: ["GET", "HEAD", "OPTIONS", "PUT", "PATCH", "POST", "DELETE"],
            cachedMethods: ["GET", "HEAD", "OPTIONS"],
            // Keep every other API route uncached; auth and authorization remain application-owned.
            cachePolicyId: CLOUDFRONT_CACHING_DISABLED_POLICY_ID,
            compress: true,
            originRequestPolicyId: CLOUDFRONT_ALL_VIEWER_ORIGIN_REQUEST_POLICY_ID,
            pathPattern: "/v1/*",
            targetOriginId: originId,
            viewerProtocolPolicy: "redirect-to-https",
          },
        ],
        comment: `${props.config.stage} api`,
        customErrorResponses: [400, 403, 404, 405, 414, 500, 501, 502, 503, 504].map((errorCode) => ({
          errorCode,
          errorCachingMinTtl: 0,
        })),
        defaultCacheBehavior: {
          allowedMethods: ["GET", "HEAD", "OPTIONS", "PUT", "PATCH", "POST", "DELETE"],
          cachedMethods: ["GET", "HEAD"],
          cachePolicyId: CLOUDFRONT_CACHING_DISABLED_POLICY_ID,
          compress: true,
          originRequestPolicyId: CLOUDFRONT_ALL_VIEWER_ORIGIN_REQUEST_POLICY_ID,
          targetOriginId: originId,
          viewerProtocolPolicy: "redirect-to-https",
        },
        enabled: true,
        httpVersion: "http2",
        ipv6Enabled: true,
        origins: [hostOrigin(this, props.config, originId)],
        priceClass: "PriceClass_100",
        viewerCertificate: {
          acmCertificateArn: props.config.apiCloudFrontCertificateArn,
          minimumProtocolVersion: "TLSv1.2_2021",
          sslSupportMethod: "sni-only",
        },
        webAclId: webAclArn,
      },
    });
  }

  private configureCloudFrontWebAcl(props: ApiCloudFrontProps): string {
    return createCloudFrontWebAcl(this, {
      id: "ApiWebAcl",
      providerFunctionName: `api-cloudfront-web-acl-provider-${props.config.stage}`,
      description: `Aura Historia ${props.config.stage} API CloudFront Web ACL`,
      metricName: `api-cloudfront-${props.config.stage}`,
      name: `application-${props.config.stage}-api-cloudfront-web-acl`,
      customResponseBodies: cloudFrontWebAclCustomResponseBodies(),
      rules: cloudFrontFreePlanWebAclRules(),
    });
  }
}

/**
 * Edge request-body limit; must equal the Axum whole-request limit (`MAX_REQUEST_BODY_BYTES`).
 * It applies to every path, provider webhooks included, so a route-specific Axum limit must not
 * exceed it.
 */
export const API_MAX_REQUEST_BODY_BYTES = 1_048_576;

/**
 * Matches decimal `Content-Length` values strictly greater than `API_MAX_REQUEST_BODY_BYTES`:
 * eight or more digits, `2000000`-`9999999`, then `1048577`-`1999999` by decreasing prefix.
 * WAF cannot compare header values numerically or inspect bodies beyond its inspection window.
 */
export const API_OVERSIZED_CONTENT_LENGTH_PATTERN =
  "^0*(?:[1-9][0-9]{7,}|[2-9][0-9]{6}|1[1-9][0-9]{5}|10[5-9][0-9]{4}|1049[0-9]{3}|1048[6-9][0-9]{2}|10485[89][0-9]|104857[7-9])$";

/**
 * Exact paths whose body or query legitimately carries URLs of internal-looking hosts, exempt
 * from the counted URL-target rules:
 * - OAuth redirect URIs or URL-valued client IDs: client registration/update, consent metadata
 *   for metadata document clients, authorize and token exchange. The API fetches only a
 *   submitted `client_id` metadata document, through its own outbound policy (HTTPS on port 443
 *   to public addresses), never a submitted redirect URI.
 * - Content notice submission: a notice cites the location of reported content, which can be a
 *   raw IP address. The API stores those URLs for human review and never fetches them.
 * The URI path is matched raw, so encoded, dot-segment or trailing-slash variants stay out of scope.
 */
export const API_URL_TARGET_EXEMPT_PATH_PATTERN =
  "^/v1/(?:content-notices|admin/oauth-clients(?:/oc_[0-9a-z]{26})?|oauth/(?:authorize|token|metadata-document-clients))$";

/**
 * Common-rule-set rules for URLs that target internal hosts in the body or query. Contract-valid
 * loopback HTTPS callbacks can match them: `https://localhost:3458/...` the EC2 metadata SSRF
 * rules, IPv4 literals such as `https://127.0.0.1:3458/...` also the RFI rules. They are counted
 * in the group and blocked outside the exempt paths.
 */
export const API_SCOPED_URL_TARGET_RULES = [
  { name: "EC2MetaDataSSRF_BODY", label: "awswaf:managed:aws:core-rule-set:EC2MetaDataSSRF_Body" },
  { name: "EC2MetaDataSSRF_QUERYARGUMENTS", label: "awswaf:managed:aws:core-rule-set:EC2MetaDataSSRF_QueryArguments" },
  { name: "GenericRFI_BODY", label: "awswaf:managed:aws:core-rule-set:GenericRFI_Body" },
  { name: "GenericRFI_QUERYARGUMENTS", label: "awswaf:managed:aws:core-rule-set:GenericRFI_QueryArguments" },
] as const;

const PAYLOAD_TOO_LARGE_RESPONSE_BODY_KEY = "payload-too-large";

function cloudFrontWebAclCustomResponseBodies(): Record<string, unknown> {
  return {
    [PAYLOAD_TOO_LARGE_RESPONSE_BODY_KEY]: {
      Content: JSON.stringify({ status: 413, title: "Payload Too Large", error: "BAD_BODY_VALUE" }),
      ContentType: "APPLICATION_JSON",
    },
  };
}

function cloudFrontFreePlanWebAclRules(): unknown[] {
  return [
    cloudFrontManagedRule("AWSManagedRulesAmazonIpReputationList", 0),
    // SizeRestrictions_BODY blocks at 8 KiB, below contract-valid JSON bodies; the
    // request-body limit rule below enforces the application limit instead.
    cloudFrontManagedRule("AWSManagedRulesCommonRuleSet", 1, [
      {
        ActionToUse: { Count: {} },
        Name: "NoUserAgent_HEADER",
      },
      {
        ActionToUse: { Count: {} },
        Name: "SizeRestrictions_BODY",
      },
      ...API_SCOPED_URL_TARGET_RULES.map(({ name }) => ({
        ActionToUse: { Count: {} },
        Name: name,
      })),
    ]),
    cloudFrontScopedUrlTargetRule(2),
    cloudFrontManagedRule("AWSManagedRulesKnownBadInputsRuleSet", 3),
    cloudFrontRequestBodyLimitRule(4),
  ];
}

/** Blocks requests the counted URL-target rules labeled, except on the exempt paths. */
function cloudFrontScopedUrlTargetRule(priority: number): unknown {
  // The name predates the content-notice exemption; it stays for metric continuity.
  const name = "API-UrlTargetOutsideOAuthRedirects";
  return {
    Action: { Block: {} },
    Name: name,
    Priority: priority,
    Statement: {
      AndStatement: {
        Statements: [
          {
            OrStatement: {
              Statements: API_SCOPED_URL_TARGET_RULES.map(({ label }) => ({
                LabelMatchStatement: { Key: label, Scope: "LABEL" },
              })),
            },
          },
          {
            NotStatement: {
              Statement: {
                RegexMatchStatement: {
                  FieldToMatch: { UriPath: {} },
                  RegexString: API_URL_TARGET_EXEMPT_PATH_PATTERN,
                  TextTransformations: [{ Priority: 0, Type: "NONE" }],
                },
              },
            },
          },
        ],
      },
    },
    VisibilityConfig: {
      CloudWatchMetricsEnabled: true,
      MetricName: name,
      SampledRequestsEnabled: true,
    },
  };
}

function cloudFrontRequestBodyLimitRule(priority: number): unknown {
  const name = "API-RequestBodyLimit";
  return {
    Action: {
      Block: {
        CustomResponse: {
          CustomResponseBodyKey: PAYLOAD_TOO_LARGE_RESPONSE_BODY_KEY,
          ResponseCode: 413,
        },
      },
    },
    Name: name,
    Priority: priority,
    Statement: {
      RegexMatchStatement: {
        FieldToMatch: { SingleHeader: { Name: "content-length" } },
        RegexString: API_OVERSIZED_CONTENT_LENGTH_PATTERN,
        TextTransformations: [{ Priority: 0, Type: "NONE" }],
      },
    },
    VisibilityConfig: {
      CloudWatchMetricsEnabled: true,
      MetricName: name,
      SampledRequestsEnabled: true,
    },
  };
}

function cloudFrontManagedRule(name: string, priority: number, ruleActionOverrides?: unknown[]): unknown {
  return {
    Name: `AWS-${name}`,
    OverrideAction: { None: {} },
    Priority: priority,
    Statement: {
      ManagedRuleGroupStatement: {
        Name: name,
        ...(ruleActionOverrides ? { RuleActionOverrides: ruleActionOverrides } : {}),
        VendorName: "AWS",
      },
    },
    VisibilityConfig: {
      CloudWatchMetricsEnabled: true,
      MetricName: `AWS-${name}`,
      SampledRequestsEnabled: true,
    },
  };
}

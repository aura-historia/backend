import * as cloudfront from "aws-cdk-lib/aws-cloudfront";
import { Construct } from "constructs";
import type { StageConfig } from "../config";
import { hostOrigin } from "./host-origin";
import { createMcpWebAcl } from "./mcp-web-acl";

/** The public, credential-free Server Card; the only MCP path that may be cached. */
export const MCP_SERVER_CARD_PATH = "/.well-known/mcp/server-card.json";

interface McpCloudFrontProps {
  readonly config: StageConfig;
}

/** The MCP host's CloudFront distribution and Web ACL in front of the host origin. */
export class McpCloudFront extends Construct {
  readonly distribution: cloudfront.CfnDistribution;
  readonly endpointUrl: string;

  constructor(scope: Construct, id: string, props: McpCloudFrontProps) {
    super(scope, id);
    this.distribution = createDistribution(this, props.config);
    this.endpointUrl = `https://${props.config.mcp.domainName}/v1`;
  }
}

function createDistribution(scope: Construct, config: StageConfig): cloudfront.CfnDistribution {
  const responseHeaders = new cloudfront.CfnResponseHeadersPolicy(scope, "NoStoreResponseHeaders", {
    responseHeadersPolicyConfig: {
      name: `mcp-${config.stage}-no-store`,
      customHeadersConfig: { items: [{ header: "Cache-Control", value: "no-store", override: true }] },
    },
  });
  const originId = "McpHttpApiOrigin";
  return new cloudfront.CfnDistribution(scope, "Distribution", {
    distributionConfig: {
      aliases: [config.mcp.domainName],
      comment: `${config.stage} mcp`,
      enabled: true,
      httpVersion: "http2",
      ipv6Enabled: true,
      priceClass: "PriceClass_100",
      cacheBehaviors: [serverCardBehavior(scope, config, originId)],
      defaultCacheBehavior: {
        allowedMethods: ["GET", "HEAD", "OPTIONS", "PUT", "PATCH", "POST", "DELETE"],
        cachedMethods: ["GET", "HEAD"],
        cachePolicyId: cloudfront.CachePolicy.CACHING_DISABLED.cachePolicyId,
        originRequestPolicyId: cloudfront.OriginRequestPolicy.ALL_VIEWER.originRequestPolicyId,
        responseHeadersPolicyId: responseHeaders.ref,
        targetOriginId: originId,
        viewerProtocolPolicy: "redirect-to-https",
      },
      customErrorResponses: [400, 403, 404, 405, 414, 416, 500, 501, 502, 503, 504]
        .map((errorCode) => ({ errorCode, errorCachingMinTtl: 0 })),
      origins: [hostOrigin(scope, config, originId)],
      viewerCertificate: {
        acmCertificateArn: config.mcp.cloudFrontCertificateArn,
        minimumProtocolVersion: "TLSv1.2_2021", sslSupportMethod: "sni-only",
      },
      webAclId: createMcpWebAcl(scope, config.stage),
    },
  });
}

/**
 * The card is identical for every viewer and ignores credentials, so the edge honors its
 * short origin TTL and shields the MCP server from discovery traffic. The server's CORS keeps
 * the exact protocol origins; this behavior alone lets any origin read the card. The server
 * always sets Cache-Control; the non-overriding no-store covers errors the proxy generates.
 */
function serverCardBehavior(scope: Construct, config: StageConfig, originId: string): cloudfront.CfnDistribution.CacheBehaviorProperty {
  const cachePolicy = new cloudfront.CfnCachePolicy(scope, "ServerCardCachePolicy", {
    cachePolicyConfig: {
      name: `mcp-${config.stage}-server-card`,
      comment: `${config.stage} MCP server card`,
      minTtl: 0, defaultTtl: 0, maxTtl: 300,
      parametersInCacheKeyAndForwardedToOrigin: {
        cookiesConfig: { cookieBehavior: "none" },
        enableAcceptEncodingBrotli: true,
        enableAcceptEncodingGzip: true,
        headersConfig: { headerBehavior: "none" },
        queryStringsConfig: { queryStringBehavior: "none" },
      },
    },
  });
  const responseHeaders = new cloudfront.CfnResponseHeadersPolicy(scope, "ServerCardResponseHeaders", {
    responseHeadersPolicyConfig: {
      name: `mcp-${config.stage}-server-card`,
      customHeadersConfig: { items: [{ header: "Cache-Control", value: "no-store", override: false }] },
      corsConfig: {
        accessControlAllowCredentials: false,
        accessControlAllowHeaders: { items: ["*"] },
        accessControlAllowMethods: { items: ["GET", "HEAD", "OPTIONS"] },
        accessControlAllowOrigins: { items: ["*"] },
        originOverride: true,
      },
    },
  });
  return {
    pathPattern: MCP_SERVER_CARD_PATH,
    allowedMethods: ["GET", "HEAD", "OPTIONS"],
    cachedMethods: ["GET", "HEAD"],
    cachePolicyId: cachePolicy.ref,
    compress: true,
    originRequestPolicyId: cloudfront.OriginRequestPolicy.ALL_VIEWER.originRequestPolicyId,
    responseHeadersPolicyId: responseHeaders.ref,
    targetOriginId: originId,
    viewerProtocolPolicy: "redirect-to-https",
  };
}

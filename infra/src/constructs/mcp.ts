import * as cdk from "aws-cdk-lib";
import * as apigwv2 from "aws-cdk-lib/aws-apigatewayv2";
import * as integrations from "aws-cdk-lib/aws-apigatewayv2-integrations";
import * as cloudfront from "aws-cdk-lib/aws-cloudfront";
import type * as lambda from "aws-cdk-lib/aws-lambda";
import * as logs from "aws-cdk-lib/aws-logs";
import { Construct } from "constructs";
import type { StageConfig } from "../config";
import { createMcpWebAcl } from "./mcp-web-acl";

/** Exact paths only. ANY /v1 lets the transport return its deliberate 405/Allow response. */
export const MCP_ROUTE_CATALOG = [
  { method: apigwv2.HttpMethod.ANY, path: "/v1" },
  ...["/health", "/ready", "/.well-known/oauth-protected-resource", "/.well-known/oauth-protected-resource/v1"]
    .flatMap((path) => [apigwv2.HttpMethod.GET, apigwv2.HttpMethod.HEAD].map((method) => ({ method, path }))),
] as const;

export const MCP_CORS_ALLOW_HEADERS = [
  "Authorization", "Content-Type", "Accept", "MCP-Protocol-Version", "Mcp-Method", "Mcp-Name",
  "Mcp-Session-Id", "X-Correlation-Id",
];
export const MCP_CORS_EXPOSE_HEADERS = ["WWW-Authenticate", "X-Request-Id", "X-Correlation-Id", "Content-Type"];

interface McpHttpApiProps {
  readonly config: StageConfig;
  readonly function: lambda.IFunction;
}

export class McpHttpApi extends Construct {
  readonly api: apigwv2.HttpApi;
  readonly stage: apigwv2.HttpStage;
  readonly distribution: cloudfront.CfnDistribution;
  readonly endpointUrl: string;

  constructor(scope: Construct, id: string, props: McpHttpApiProps) {
    super(scope, id);
    const { api, stage } = createGateway(this, props);
    this.api = api;
    this.stage = stage;
    const domain = createRegionalDomain(this, props.config, api, stage);
    this.distribution = createDistribution(this, props.config, domain);
    this.endpointUrl = `https://${props.config.mcp.domainName}/v1`;
  }
}

function createGateway(scope: Construct, props: McpHttpApiProps) {
  const { config } = props;
  const api = new apigwv2.HttpApi(scope, "Api", {
    apiName: `mcp-${config.stage}`,
    createDefaultStage: false,
    disableExecuteApiEndpoint: true,
    corsPreflight: {
      allowOrigins: config.mcp.allowedOrigins,
      allowMethods: [apigwv2.CorsHttpMethod.GET, apigwv2.CorsHttpMethod.HEAD, apigwv2.CorsHttpMethod.POST, apigwv2.CorsHttpMethod.OPTIONS],
      allowHeaders: MCP_CORS_ALLOW_HEADERS,
      exposeHeaders: MCP_CORS_EXPOSE_HEADERS,
      maxAge: cdk.Duration.seconds(0),
    },
  });
  const logGroup = new logs.LogGroup(scope, "AccessLogs", {
    logGroupName: `/aws/apigateway/mcp-${config.stage}`,
    retention: logs.RetentionDays.ONE_WEEK,
    removalPolicy: config.removalPolicy,
  });
  const stage = new apigwv2.HttpStage(scope, "Stage", {
    httpApi: api,
    stageName: config.stage,
    autoDeploy: true,
    throttle: config.isProd ? { burstLimit: 10, rateLimit: 20 } : { burstLimit: 4, rateLimit: 5 },
  });
  (stage.node.defaultChild as apigwv2.CfnStage).accessLogSettings = {
    destinationArn: logGroup.logGroupArn,
    format: JSON.stringify({
      requestId: "$context.requestId", routeKey: "$context.routeKey", status: "$context.status",
      integrationLatency: "$context.integrationLatency",
    }),
  };
  const integration = new integrations.HttpLambdaIntegration("McpIntegration", props.function, {
    scopePermissionToRoute: false,
    timeout: cdk.Duration.seconds(29),
  });
  for (const route of MCP_ROUTE_CATALOG) {
    api.addRoutes({ path: route.path, methods: [route.method], integration });
  }
  return { api, stage };
}

function createRegionalDomain(
  scope: Construct, config: StageConfig, api: apigwv2.HttpApi, stage: apigwv2.HttpStage,
): apigwv2.CfnDomainName {
  const domain = new apigwv2.CfnDomainName(scope, "DomainName", {
    domainName: config.mcp.domainName,
    domainNameConfigurations: [{
      certificateArn: config.mcp.gatewayCertificateArn,
      endpointType: "REGIONAL",
      securityPolicy: "TLS_1_2",
    }],
    routingMode: "API_MAPPING_ONLY",
  });
  const mapping = new apigwv2.CfnApiMapping(scope, "DomainMapping", {
    apiId: api.apiId, domainName: domain.ref, stage: stage.stageName,
  });
  mapping.addDependency(domain);
  mapping.addDependency(stage.node.defaultChild as apigwv2.CfnStage);
  return domain;
}

function createDistribution(scope: Construct, config: StageConfig, domain: apigwv2.CfnDomainName) {
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
      origins: [{
        id: originId,
        domainName: domain.attrRegionalDomainName,
        customOriginConfig: {
          httpPort: 80, httpsPort: 443, originProtocolPolicy: "https-only",
          originKeepaliveTimeout: 5, originReadTimeout: 30, originSslProtocols: ["TLSv1.2"],
        },
      }],
      viewerCertificate: {
        acmCertificateArn: config.mcp.cloudFrontCertificateArn,
        minimumProtocolVersion: "TLSv1.2_2021", sslSupportMethod: "sni-only",
      },
      webAclId: createMcpWebAcl(scope, config.stage),
    },
  });
}

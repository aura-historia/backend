import * as cdk from "aws-cdk-lib";
import * as apigwv2 from "aws-cdk-lib/aws-apigatewayv2";
import * as integrations from "aws-cdk-lib/aws-apigatewayv2-integrations";
import * as cloudfront from "aws-cdk-lib/aws-cloudfront";
import * as iam from "aws-cdk-lib/aws-iam";
import * as lambda from "aws-cdk-lib/aws-lambda";
import * as logs from "aws-cdk-lib/aws-logs";
import * as fs from "node:fs";
import * as path from "node:path";
import { Construct } from "constructs";
import type { StageConfig } from "../config";

import type { LambdaCatalog, LambdaKey } from "./lambdas";

export enum RouteAuthPolicy {
  Anonymous = "ANONYMOUS",
  OptionalBearer = "OPTIONAL_BEARER",
  ApplicationBearer = "APPLICATION_BEARER",
  OAuth = "OAUTH",
  ProviderSignature = "PROVIDER_SIGNATURE",
}

export interface RouteDefinition {
  readonly method: apigwv2.HttpMethod;
  readonly path: string;
  readonly lambda: LambdaKey;
  /** The application owns these policies because routes accept Aura opaque tokens and provider credentials. */
  readonly auth: RouteAuthPolicy;
}

const CLOUDFRONT_CACHING_DISABLED_POLICY_ID = "4135ea2d-6df8-44a3-9df3-4b5a84be39ad";
const CLOUDFRONT_ALL_VIEWER_ORIGIN_REQUEST_POLICY_ID = "216adef6-5c7f-47e4-b989-5492eafa07d3";

const apiRoutes = (
  auth: RouteAuthPolicy,
  path: string,
  methods: readonly (keyof typeof apigwv2.HttpMethod)[],
): RouteDefinition[] => methods.map((method) => route(method, path, "auraHistoriaApi", auth));

/**
 * Closed HTTP API policy matrix. It is kept in lockstep with Axum and OpenAPI by
 * `test/api-route-matrix.test.ts`; adding an Axum or OpenAPI operation without an
 * entry here cannot expose it through API Gateway.
 */
export const API_ROUTE_CATALOG: readonly RouteDefinition[] = [
  ...apiRoutes(RouteAuthPolicy.Anonymous, "/health", ["GET"]),
  ...apiRoutes(RouteAuthPolicy.Anonymous, "/ready", ["GET"]),

  ...apiRoutes(RouteAuthPolicy.OptionalBearer, "/api/v1/auctions", ["GET"]),
  ...apiRoutes(RouteAuthPolicy.OptionalBearer, "/api/v1/auctions/{auction_id}", ["GET"]),
  ...apiRoutes(RouteAuthPolicy.OptionalBearer, "/api/v1/auctions/{auction_id}/product-listings", ["GET"]),
  ...apiRoutes(RouteAuthPolicy.OptionalBearer, "/api/v1/product-listings", ["GET"]),
  ...apiRoutes(RouteAuthPolicy.OptionalBearer, "/api/v1/product-listings/by-slug/{product_listing_title_slug_id}", ["GET"]),
  ...apiRoutes(RouteAuthPolicy.OptionalBearer, "/api/v1/product-listings/{product_listing_id}", ["GET"]),
  ...apiRoutes(RouteAuthPolicy.OptionalBearer, "/api/v1/product-listings/{product_listing_id}/history", ["GET"]),
  ...apiRoutes(RouteAuthPolicy.OptionalBearer, "/api/v1/product-listings/{product_listing_id}/similar", ["GET"]),
  ...apiRoutes(RouteAuthPolicy.OptionalBearer, "/api/v1/listing-sources", ["GET"]),
  ...apiRoutes(RouteAuthPolicy.OptionalBearer, "/api/v1/listing-sources/by-slug/{listing_source_slug_id}", ["GET"]),
  ...apiRoutes(RouteAuthPolicy.OptionalBearer, "/api/v1/newsletter-subscriptions", ["PUT"]),

  ...apiRoutes(RouteAuthPolicy.ApplicationBearer, "/api/v1/listing-sources/{listing_source_id}/product-listings", ["POST", "PATCH", "PUT", "DELETE"]),
  ...apiRoutes(RouteAuthPolicy.ApplicationBearer, "/api/v1/admin/auctions", ["POST"]),
  ...apiRoutes(RouteAuthPolicy.ApplicationBearer, "/api/v1/admin/auctions/{auction_id}", ["GET", "PATCH"]),
  ...apiRoutes(RouteAuthPolicy.ApplicationBearer, "/api/v1/admin/overview", ["GET"]),
  ...apiRoutes(RouteAuthPolicy.ApplicationBearer, "/api/v1/admin/listing-sources", ["GET", "POST"]),
  ...apiRoutes(RouteAuthPolicy.ApplicationBearer, "/api/v1/admin/listing-sources/{listing_source_id}", ["GET", "PATCH", "DELETE"]),
  ...apiRoutes(RouteAuthPolicy.ApplicationBearer, "/api/v1/me/listing-sources", ["GET"]),
  ...apiRoutes(RouteAuthPolicy.ApplicationBearer, "/api/v1/admin/parties", ["GET", "POST"]),
  ...apiRoutes(RouteAuthPolicy.ApplicationBearer, "/api/v1/admin/parties/{party_id}", ["GET", "PATCH", "DELETE"]),
  ...apiRoutes(RouteAuthPolicy.ApplicationBearer, "/api/v1/me", ["DELETE"]),
  ...apiRoutes(RouteAuthPolicy.ApplicationBearer, "/api/v1/me/account", ["GET", "PATCH"]),
  ...apiRoutes(RouteAuthPolicy.ApplicationBearer, "/api/v1/me/access-tokens", ["GET", "POST", "PATCH"]),
  ...apiRoutes(RouteAuthPolicy.ApplicationBearer, "/api/v1/me/access-tokens/{access_token_id}", ["GET", "DELETE"]),
  ...apiRoutes(RouteAuthPolicy.ApplicationBearer, "/api/v1/me/watchlist", ["GET", "POST"]),
  ...apiRoutes(RouteAuthPolicy.ApplicationBearer, "/api/v1/me/watchlist/{product_listing_id}", ["PATCH", "DELETE"]),
  ...apiRoutes(RouteAuthPolicy.ApplicationBearer, "/api/v1/me/search-filters", ["GET", "POST"]),
  ...apiRoutes(RouteAuthPolicy.ApplicationBearer, "/api/v1/me/search-filters/{user_search_filter_id}", ["GET", "PATCH", "DELETE"]),
  ...apiRoutes(RouteAuthPolicy.ApplicationBearer, "/api/v1/me/search-filters/{user_search_filter_id}/matches", ["GET"]),
  ...apiRoutes(RouteAuthPolicy.ApplicationBearer, "/api/v1/me/search-filters/{user_search_filter_id}/matches/{product_listing_id}", ["PATCH"]),
  ...apiRoutes(RouteAuthPolicy.ApplicationBearer, "/api/v1/me/notifications", ["GET", "PATCH", "DELETE"]),
  ...apiRoutes(RouteAuthPolicy.ApplicationBearer, "/api/v1/me/notifications/all", ["PATCH"]),
  ...apiRoutes(RouteAuthPolicy.ApplicationBearer, "/api/v1/me/notifications/{notification_id}", ["PATCH", "DELETE"]),
  ...apiRoutes(RouteAuthPolicy.ApplicationBearer, "/api/v1/me/billing/checkout", ["POST"]),
  ...apiRoutes(RouteAuthPolicy.ApplicationBearer, "/api/v1/me/billing/portal", ["POST"]),
  ...apiRoutes(RouteAuthPolicy.ApplicationBearer, "/api/v1/me/billing/manage", ["POST"]),
  ...apiRoutes(RouteAuthPolicy.ApplicationBearer, "/api/v1/me/partnership-applications", ["GET", "POST"]),
  ...apiRoutes(RouteAuthPolicy.ApplicationBearer, "/api/v1/me/partnership-applications/{partnership_application_id}", ["GET", "DELETE"]),
  ...apiRoutes(RouteAuthPolicy.ApplicationBearer, "/api/v1/admin/partnership-applications", ["GET"]),
  ...apiRoutes(RouteAuthPolicy.ApplicationBearer, "/api/v1/admin/partnership-applications/{partnership_application_id}", ["GET", "PATCH"]),
  ...apiRoutes(RouteAuthPolicy.ApplicationBearer, "/api/v1/admin/partnership-applications/{partnership_application_id}/decision", ["POST"]),
  ...apiRoutes(RouteAuthPolicy.ApplicationBearer, "/api/v1/admin/partnerships", ["GET"]),
  ...apiRoutes(RouteAuthPolicy.ApplicationBearer, "/api/v1/admin/partnerships/{partnership_id}", ["GET", "DELETE"]),
  ...apiRoutes(RouteAuthPolicy.ApplicationBearer, "/api/v1/admin/partnerships/{partnership_id}/members/{user_id}", ["PUT", "DELETE"]),
  ...apiRoutes(RouteAuthPolicy.ApplicationBearer, "/api/v1/admin/partnerships/{partnership_id}/listing-source-grants/{listing_source_id}", ["PUT", "DELETE"]),
  ...apiRoutes(RouteAuthPolicy.ApplicationBearer, "/api/v1/admin/users", ["GET"]),
  ...apiRoutes(RouteAuthPolicy.ApplicationBearer, "/api/v1/admin/users/{user_id}", ["GET", "PATCH", "DELETE"]),
  ...apiRoutes(RouteAuthPolicy.ApplicationBearer, "/api/v1/admin/users/{user_id}/suspension", ["PUT", "DELETE"]),
  ...apiRoutes(RouteAuthPolicy.ApplicationBearer, "/api/v1/admin/users/{user_id}/sessions/revoke", ["POST"]),
  ...apiRoutes(RouteAuthPolicy.ApplicationBearer, "/api/v1/admin/users/{user_id}/access-tokens", ["GET", "DELETE"]),
  ...apiRoutes(RouteAuthPolicy.ApplicationBearer, "/api/v1/admin/users/{user_id}/access-tokens/{access_token_id}", ["DELETE"]),
  ...apiRoutes(RouteAuthPolicy.ApplicationBearer, "/api/v1/admin/oauth-clients", ["GET", "POST"]),
  ...apiRoutes(RouteAuthPolicy.ApplicationBearer, "/api/v1/admin/oauth-clients/{client_id}", ["GET", "PATCH", "DELETE"]),

  ...apiRoutes(RouteAuthPolicy.OAuth, "/api/v1/oauth/authorize", ["GET"]),
  ...apiRoutes(RouteAuthPolicy.OAuth, "/api/v1/oauth/token", ["POST"]),
  ...apiRoutes(RouteAuthPolicy.OAuth, "/api/v1/oauth/tokens/by-third-party-code/{third_party_code}", ["GET"]),
  ...apiRoutes(RouteAuthPolicy.OAuth, "/api/v1/oauth/revoke", ["POST"]),
  ...apiRoutes(RouteAuthPolicy.OAuth, "/api/v1/oauth/introspect", ["POST"]),

  ...apiRoutes(RouteAuthPolicy.ProviderSignature, "/api/v1/webhooks/woocommerce/{listing_source_id}", ["POST"]),
];

export interface HttpApiProps {
  readonly config: StageConfig;
  readonly stageName: string;
  readonly functions: LambdaCatalog;
}

export class BackendHttpApi extends Construct {
  readonly api: apigwv2.HttpApi;
  readonly stage: apigwv2.HttpStage;
  readonly distribution?: cloudfront.CfnDistribution;
  readonly endpointUrl: string;

  constructor(scope: Construct, id: string, props: HttpApiProps) {
    super(scope, id);

    this.api = new apigwv2.HttpApi(this, "Api", {
      apiName: `api-${props.stageName}`,
      createDefaultStage: false,
      corsPreflight: {
        allowHeaders: [
          "Authorization",
          "Content-Type",
          "Accept",
          "X-Correlation-Id",
          "X-WC-Webhook-Source",
          "X-WC-Webhook-Topic",
          "X-WC-Webhook-Signature",
          "X-WC-Webhook-Resource",
          "X-WC-Webhook-Event",
          "X-WC-Webhook-ID",
          "X-WC-Webhook-Delivery-ID",
          "X-Api-Key",
        ],
        allowMethods: [apigwv2.CorsHttpMethod.ANY],
        allowOrigins: props.config.apiCorsAllowOrigins,
      },
    });

    if (props.config.apiDomainName) {
      const cfnApi = this.api.node.defaultChild as apigwv2.CfnApi;
      cfnApi.addPropertyOverride("DisableExecuteApiEndpoint", true);
    }

    const logGroup = props.config.enableProductionObservability
      ? new logs.LogGroup(this, "ApiLogGroup", {
          logGroupName: `/aws/apigateway/api-${props.stageName}`,
          retention: logs.RetentionDays.ONE_WEEK,
          removalPolicy: props.config.removalPolicy,
        })
      : undefined;

    this.stage = new apigwv2.HttpStage(this, "ApiStage", {
      httpApi: this.api,
      stageName: props.stageName,
      autoDeploy: true,
      throttle: props.config.enableProductionObservability
        ? { burstLimit: 5000, rateLimit: 2000 }
        : { burstLimit: 50, rateLimit: 20 },
    });

    if (logGroup) {
      const cfnStage = this.stage.node.defaultChild as apigwv2.CfnStage;
      cfnStage.addPropertyOverride("AccessLogSettings", {
        DestinationArn: logGroup.logGroupArn,
        Format: JSON.stringify({
          requestId: "$context.requestId",
          ip: "$context.identity.sourceIp",
          requestTime: "$context.requestTime",
          httpMethod: "$context.httpMethod",
          routeKey: "$context.routeKey",
          status: "$context.status",
          protocol: "$context.protocol",
          responseLength: "$context.responseLength",
          integrationLatency: "$context.integrationLatency",
          responseLatency: "$context.responseLatency",
          integrationStatus: "$context.integrationStatus",
          errorMessage: "$context.error.message",
          errorMessageString: "$context.error.messageString",
        }),
      });
      cfnStage.addPropertyOverride("DefaultRouteSettings.DetailedMetricsEnabled", true);
    }


    const integrationsByLambda = new Map<LambdaKey, integrations.HttpLambdaIntegration>();
    const localStackPathParameterLambdas = new Map<LambdaKey, NonNullable<LambdaCatalog[LambdaKey]>>();
    for (const definition of API_ROUTE_CATALOG) {
      const targetFunction = props.functions[definition.lambda];
      if (!targetFunction) {
        throw new Error(`No Lambda function configured for route '${definition.method} ${definition.path}'`);
      }

      let integration = integrationsByLambda.get(definition.lambda);
      if (!integration) {
        integration = new integrations.HttpLambdaIntegration(
          `${definition.lambda}Integration`,
          targetFunction,
        );
        integrationsByLambda.set(definition.lambda, integration);
      }

      this.api.addRoutes({
        path: definition.path,
        methods: [definition.method],
        integration,
      });

      if (props.config.isEphemeral && definition.path.includes("{")) {
        localStackPathParameterLambdas.set(definition.lambda, targetFunction);
      }
    }

    this.grantLocalStackPathParameterInvokes(localStackPathParameterLambdas);

    const customDomain = this.configureCustomDomain(props);
    this.distribution = this.configureCloudFront(props, customDomain);

    this.endpointUrl = props.config.apiEndpointUrl ?? `${this.api.apiEndpoint}/${props.stageName}`;
  }

  private grantLocalStackPathParameterInvokes(functions: Map<LambdaKey, NonNullable<LambdaCatalog[LambdaKey]>>): void {
    for (const [lambdaKey, targetFunction] of functions) {
      targetFunction.addPermission(`${lambdaKey}LocalStackPathParameterInvoke`, {
        principal: new iam.ServicePrincipal("apigateway.amazonaws.com"),
        sourceArn: this.api.arnForExecuteApi("*", "/*"),
      });
    }
  }

  private configureCustomDomain(props: HttpApiProps): apigwv2.CfnDomainName | undefined {
    if (!props.config.apiDomainName || !props.config.apiGatewayCertificateArn) {
      return undefined;
    }

    const domain = new apigwv2.CfnDomainName(this, "ApiDomainName", {
      domainName: props.config.apiDomainName,
      domainNameConfigurations: [
        {
          certificateArn: props.config.apiGatewayCertificateArn,
          endpointType: "REGIONAL",
          securityPolicy: "TLS_1_2",
        },
      ],
      routingMode: "API_MAPPING_ONLY",
    });

    const mapping = new apigwv2.CfnApiMapping(this, "ApiDomainMapping", {
      apiId: this.api.apiId,
      domainName: domain.ref,
      stage: this.stage.stageName,
    });
    mapping.addDependency(domain);
    mapping.addDependency(this.stage.node.defaultChild as apigwv2.CfnStage);

    return domain;
  }

  private configureCloudFront(
    props: HttpApiProps,
    customDomain: apigwv2.CfnDomainName | undefined,
  ): cloudfront.CfnDistribution | undefined {
    if (!props.config.apiDomainName || !props.config.apiCloudFrontCertificateArn || !customDomain) {
      return undefined;
    }


    const originId = "HttpApiOrigin";
    const webAclArn = this.configureCloudFrontWebAcl(props);

    return new cloudfront.CfnDistribution(this, "ApiDistribution", {
      distributionConfig: {
        aliases: props.config.apiCloudFrontAliases,
        cacheBehaviors: [
          {
            allowedMethods: ["GET", "HEAD", "OPTIONS", "PUT", "PATCH", "POST", "DELETE"],
            cachedMethods: ["GET", "HEAD", "OPTIONS"],
            // API responses can be personalized by optional Aura/Cognito credentials.
            // Disable shared edge caching rather than relying on a token-derived cache key.
            cachePolicyId: CLOUDFRONT_CACHING_DISABLED_POLICY_ID,
            compress: true,
            originRequestPolicyId: CLOUDFRONT_ALL_VIEWER_ORIGIN_REQUEST_POLICY_ID,
            pathPattern: "/api/*",
            targetOriginId: originId,
            viewerProtocolPolicy: "redirect-to-https",
          },
        ],
        comment: `${props.stageName} api`,
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
        origins: [
          {
            id: originId,
            domainName: customDomain.attrRegionalDomainName,
            customOriginConfig: {
              httpPort: 80,
              httpsPort: 443,
              originKeepaliveTimeout: 5,
              originProtocolPolicy: "https-only",
              originReadTimeout: 30,
              originSslProtocols: ["TLSv1.2"],
            },
          },
        ],
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

  private configureCloudFrontWebAcl(props: HttpApiProps): string {
    const provider = new lambda.Function(this, "ApiWebAclCustomResourceFunction", {
      functionName: `api-cloudfront-web-acl-provider-${props.stageName}`,
      runtime: lambda.Runtime.NODEJS_24_X,
      handler: "index.handler",
      timeout: cdk.Duration.minutes(2),
      code: lambda.Code.fromInline(cloudFrontWebAclCustomResourceCode()),
    });

    provider.addToRolePolicy(new iam.PolicyStatement({
      actions: [
        "wafv2:CreateWebACL",
        "wafv2:DeleteWebACL",
        "wafv2:GetWebACL",
        "wafv2:ListWebACLs",
        "wafv2:TagResource",
        "wafv2:UpdateWebACL",
      ],
      resources: ["*"],
    }));

    const webAcl = new cdk.CustomResource(this, "ApiWebAcl", {
      serviceToken: provider.functionArn,
      properties: {
        Description: `Aura Historia ${props.stageName} API CloudFront Web ACL`,
        MetricName: `api-cloudfront-${props.stageName}`,
        Name: `application-${props.stageName}-api-cloudfront-web-acl`,
        Region: "us-east-1",
        Rules: cloudFrontFreePlanWebAclRules(),
        Scope: "CLOUDFRONT",
      },
    });

    return webAcl.getAttString("WebAclArn");
  }
}

function cloudFrontFreePlanWebAclRules(): unknown[] {
  return [
    cloudFrontManagedRule("AWSManagedRulesAmazonIpReputationList", 0),
    cloudFrontManagedRule("AWSManagedRulesCommonRuleSet", 1, [
      {
        ActionToUse: { Count: {} },
        Name: "NoUserAgent_HEADER",
      },
    ]),
    cloudFrontManagedRule("AWSManagedRulesKnownBadInputsRuleSet", 2),
  ];
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

function cloudFrontWebAclCustomResourceCode(): string {
  return resourceCode("api-web-acl-custom-resource.js");
}


function resourceCode(fileName: string): string {
  return fs.readFileSync(path.join(__dirname, "..", "resources", fileName), "utf8");
}

function route(
  method: keyof typeof apigwv2.HttpMethod,
  path: string,
  lambdaKey: LambdaKey,
  auth: RouteAuthPolicy,
): RouteDefinition {
  return {
    method: apigwv2.HttpMethod[method],
    path,
    lambda: lambdaKey,
    auth,
  };
}

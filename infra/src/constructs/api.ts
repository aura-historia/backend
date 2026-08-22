import * as cdk from "aws-cdk-lib";
import * as apigwv2 from "aws-cdk-lib/aws-apigatewayv2";
import * as authorizers from "aws-cdk-lib/aws-apigatewayv2-authorizers";
import * as integrations from "aws-cdk-lib/aws-apigatewayv2-integrations";
import * as cloudfront from "aws-cdk-lib/aws-cloudfront";
import * as iam from "aws-cdk-lib/aws-iam";
import * as lambda from "aws-cdk-lib/aws-lambda";
import * as logs from "aws-cdk-lib/aws-logs";
import * as fs from "node:fs";
import * as path from "node:path";
import { Construct } from "constructs";
import type { StageConfig } from "../config";
import type { Identity } from "./cognito";
import type { LambdaCatalog, LambdaKey } from "./lambdas";

interface RouteDefinition {
  readonly method: apigwv2.HttpMethod;
  readonly path: string;
  readonly lambda: LambdaKey;
  readonly authenticated?: boolean;
}

const CLOUDFRONT_CACHING_DISABLED_POLICY_ID = "4135ea2d-6df8-44a3-9df3-4b5a84be39ad";
const CLOUDFRONT_ALL_VIEWER_ORIGIN_REQUEST_POLICY_ID = "216adef6-5c7f-47e4-b989-5492eafa07d3";
const CLOUDFRONT_USE_ORIGIN_CACHE_CONTROL_QUERY_STRINGS_POLICY_ID = "4cc15a8a-d715-48a4-82b8-cc0b614638fe";

const ROUTES: readonly RouteDefinition[] = [];

export interface HttpApiProps {
  readonly config: StageConfig;
  readonly stageName: string;
  readonly functions: LambdaCatalog;
  readonly identity: Identity;
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

    const authorizer = new authorizers.HttpJwtAuthorizer(
      "ApiCognitoAuthorizer",
      cdk.Fn.sub("https://cognito-idp.${AWS::Region}.amazonaws.com/${UserPoolId}", {
        UserPoolId: props.identity.userPool.userPoolId,
      }),
      {
        jwtAudience: [props.identity.publicClient.userPoolClientId],
        identitySource: ["$request.header.Authorization"],
      },
    );

    const integrationsByLambda = new Map<LambdaKey, integrations.HttpLambdaIntegration>();
    const localStackPathParameterLambdas = new Map<LambdaKey, NonNullable<LambdaCatalog[LambdaKey]>>();
    for (const definition of ROUTES) {
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
        authorizer: definition.authenticated ? authorizer : undefined,
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

    const authCacheGuard = new cloudfront.CfnFunction(this, "ApiAuthCacheGuardFunction", {
      name: `api-guard-cache-control-no-cache-when-authenticated-${props.stageName}`,
      autoPublish: true,
      functionCode: authCacheGuardFunctionCode(),
      functionConfig: {
        comment: "Add an auth cache key for JWT requests.",
        runtime: "cloudfront-js-2.0",
      },
    });

    const originId = "HttpApiOrigin";
    const webAclArn = this.configureCloudFrontWebAcl(props);

    return new cloudfront.CfnDistribution(this, "ApiDistribution", {
      distributionConfig: {
        aliases: props.config.apiCloudFrontAliases,
        cacheBehaviors: [
          {
            allowedMethods: ["GET", "HEAD", "OPTIONS", "PUT", "PATCH", "POST", "DELETE"],
            cachedMethods: ["GET", "HEAD", "OPTIONS"],
            cachePolicyId: CLOUDFRONT_USE_ORIGIN_CACHE_CONTROL_QUERY_STRINGS_POLICY_ID,
            compress: true,
            functionAssociations: [
              {
                eventType: "viewer-request",
                functionArn: authCacheGuard.attrFunctionArn,
              },
            ],
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
      runtime: lambda.Runtime.NODEJS_20_X,
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

function authCacheGuardFunctionCode(): string {
  return resourceCode("api-auth-cache-guard.js");
}

function resourceCode(fileName: string): string {
  return fs.readFileSync(path.join(__dirname, "..", "resources", fileName), "utf8");
}

function route(
  method: keyof typeof apigwv2.HttpMethod,
  path: string,
  lambdaKey: LambdaKey,
  authenticated = false,
): RouteDefinition {
  return {
    method: apigwv2.HttpMethod[method],
    path,
    lambda: lambdaKey,
    authenticated,
  };
}

import * as cdk from "aws-cdk-lib";
import { Match, Template } from "aws-cdk-lib/assertions";
import { ApplicationEphemeralStack, createApplicationStacks, type ApplicationStageStacks } from "../src/application-stack";
import { ARTIFACT_BUCKET_NAME, STAGES, type StageName } from "../src/config";

type TemplateJson = ReturnType<Template["toJSON"]>;
type CfnResource = {
  readonly Type?: string;
  readonly Properties?: Record<string, any>;
};
type StageTemplates = {
  readonly data: Template;
  readonly compute: Template;
  readonly api: Template;
  readonly observability?: Template;
};

function createStacks(stage: StageName): ApplicationStageStacks {
  const app = new cdk.App({
    analyticsReporting: false,
  });

  return createApplicationStacks(app, {
    stage,
    stackNamePrefix: `application-${stage}`,
  });
}

function synthesize(stage: StageName): StageTemplates {
  const stacks = createStacks(stage);
  return {
    data: Template.fromStack(stacks.data),
    compute: Template.fromStack(stacks.compute),
    api: Template.fromStack(stacks.api),
    observability: stacks.observability ? Template.fromStack(stacks.observability) : undefined,
  };
}

function synthesizeSingleEphemeralStack(): Template {
  const app = new cdk.App({
    analyticsReporting: false,
  });
  return Template.fromStack(new ApplicationEphemeralStack(app, "application-ephemeral", { stage: "ephemeral" }));
}

function templateList(templates: StageTemplates): Template[] {
  return [templates.data, templates.compute, templates.api, templates.observability].filter((template): template is Template => !!template);
}

function resourcesOfType(json: TemplateJson, type: string): Array<[string, CfnResource]> {
  return Object.entries((json.Resources ?? {}) as Record<string, CfnResource>).filter(([, resource]) => resource.Type === type);
}

function allResourcesOfType(templates: StageTemplates, type: string): Array<[string, CfnResource]> {
  return templateList(templates).flatMap((template) => resourcesOfType(template.toJSON(), type));
}

function resourceCount(templates: StageTemplates, type: string): number {
  return allResourcesOfType(templates, type).length;
}

function resourcePropertiesCount(templates: StageTemplates, type: string, properties: unknown): number {
  return templateList(templates).reduce((count, template) => {
    const before = count;
    try {
      template.resourcePropertiesCountIs(type, properties, 0);
      return before;
    } catch {
      return before + resourcesOfType(template.toJSON(), type).filter(([, resource]) => {
        try {
          Template.fromJSON({ Resources: { Candidate: resource } }).hasResourceProperties(type, properties);
          return true;
        } catch {
          return false;
        }
      }).length;
    }
  }, 0);
}

function hasResourceProperties(templates: StageTemplates, type: string, properties: unknown): void {
  for (const template of templateList(templates)) {
    try {
      template.hasResourceProperties(type, properties);
      return;
    } catch {
      continue;
    }
  }

  throw new Error(`No ${type} resource matched ${JSON.stringify(properties)}`);
}

function apiCorsOrigins(json: TemplateJson): readonly string[] {
  const api = resourcesOfType(json, "AWS::ApiGatewayV2::Api")[0]?.[1];
  return api?.Properties?.CorsConfiguration?.AllowOrigins ?? [];
}

function lambdaNames(json: TemplateJson): Set<string> {
  return new Set(
    resourcesOfType(json, "AWS::Lambda::Function").map(([, resource]) => resource.Properties?.FunctionName as string),
  );
}

function lambdaMetricAlarmFunctionNames(json: TemplateJson, metricName: string): Set<string> {
  const names = new Set<string>();

  for (const [, alarm] of resourcesOfType(json, "AWS::CloudWatch::Alarm")) {
    if (alarm.Properties?.MetricName !== metricName || alarm.Properties?.Namespace !== "AWS/Lambda") {
      continue;
    }

    const functionNameDimension = (alarm.Properties?.Dimensions ?? []).find(
      (dimension: any) => dimension.Name === "FunctionName",
    );
    const value = functionNameDimension?.Value;

    if (typeof value === "string") {
      names.add(value);
    }
  }

  return names;
}

describe("Application stacks", () => {
  test("synthesizes the single ephemeral acceptance-test stack", () => {
    const template = synthesizeSingleEphemeralStack();
    const json = template.toJSON();

    expect(resourcesOfType(json, "AWS::DynamoDB::Table")).toHaveLength(1);
    expect(resourcesOfType(json, "AWS::Cognito::UserPool")).toHaveLength(1);
    expect(resourcesOfType(json, "AWS::ApiGatewayV2::Api")).toHaveLength(1);
    expect(resourcesOfType(json, "AWS::ApiGatewayV2::Route")).toHaveLength(72);
    expect(resourcesOfType(json, "AWS::CloudFront::Distribution")).toHaveLength(0);
    expect(Object.keys(json.Parameters ?? {})).toEqual(["CommitSHA"]);
    expect(json.Outputs?.ApiGatewayEndpointUrl).toBeDefined();
    expect(JSON.stringify(json)).not.toContain("Fn::ImportValue");
  });

  test("ephemeral API broadens path-parameter Lambda invokes for LocalStack", () => {
    const json = synthesizeSingleEphemeralStack().toJSON();
    const permissions = resourcesOfType(json, "AWS::Lambda::Permission").filter(([id]) =>
      id.includes("LocalStackPathParameterInvoke"),
    );

    expect(permissions).toHaveLength(10);
    for (const [, permission] of permissions) {
      expect(permission.Properties?.Principal).toBe("apigateway.amazonaws.com");
      expect(JSON.stringify(permission.Properties?.SourceArn)).toContain("/*/*/*");
    }
  });

  test.each(STAGES)("synthesizes the %s stack contract", (stage) => {
    const templates = synthesize(stage);

    expect(resourceCount(templates, "AWS::DynamoDB::Table")).toBe(1);
    expect(resourceCount(templates, "AWS::Cognito::UserPool")).toBe(1);
    expect(resourceCount(templates, "AWS::Cognito::UserPoolClient")).toBe(1);
    expect(resourceCount(templates, "AWS::ApiGatewayV2::Api")).toBe(1);
    expect(resourceCount(templates, "AWS::ApiGatewayV2::Route")).toBe(72);
    expect(resourceCount(templates, "AWS::ApiGatewayV2::Integration")).toBe(12);
    expect(resourceCount(templates, "AWS::SQS::Queue")).toBe(26);
    expect(resourceCount(templates, "AWS::Lambda::EventSourceMapping")).toBe(13);
    expect(resourceCount(templates, "AWS::StepFunctions::StateMachine")).toBe(1);
    expect(resourceCount(templates, "AWS::Pipes::Pipe")).toBe(1);

    expect(Object.keys(templates.compute.toJSON().Parameters ?? {})).toEqual(["CommitSHA"]);
    expect(templates.data.toJSON().Parameters).toBeUndefined();
    expect(templates.api.toJSON().Parameters).toBeUndefined();
    for (const template of templateList(templates)) {
      expect(template.toJSON().Resources?.CDKMetadata).toBeUndefined();
    }
    expect(templates.api.toJSON().Outputs?.ApiGatewayEndpointUrl).toBeDefined();
    if (stage === "ephemeral") {
      expect(templates.api.toJSON().Outputs?.ApiCloudFrontDistributionDomainName).toBeUndefined();
    } else {
      expect(templates.api.toJSON().Outputs?.ApiCloudFrontDistributionDomainName).toBeDefined();
    }
    expect(templates.data.toJSON().Outputs?.DynamodbTable1Name).toBeDefined();
    expect(templates.data.toJSON().Outputs?.PostgresHost).toBeDefined();
    expect(templates.data.toJSON().Outputs?.PostgresPort).toBeDefined();
    expect(templates.data.toJSON().Outputs?.PostgresDatabase).toBeDefined();
    expect(templates.compute.toJSON().Outputs?.CognitoUserPoolId).toBeDefined();
    expect(templates.compute.toJSON().Outputs?.CognitoUserPoolClientPublicId).toBeDefined();
    expect(templates.compute.toJSON().Outputs?.CognitoUserPoolClientAdminId).toBeUndefined();
    expect(templates.data.toJSON().Outputs?.NotificationSendQueueUrl).toBeDefined();
    expect(templates.compute.toJSON().Outputs?.StripeEventBusName).toBeDefined();
    expect(templates.compute.toJSON().Outputs?.ShopifyEventBusName).toBeDefined();
  });

  test.each(STAGES)("product delete worker reports partial SQS batch failures for %s", (stage) => {
    const templates = synthesize(stage);

    templates.compute.hasResourceProperties("AWS::Lambda::EventSourceMapping", {
      BatchSize: 100,
      EventSourceArn: {
        "Fn::Join": [
          "",
          Match.arrayWith([`:product-lambda-delete-product-queue-${stage}`]),
        ],
      },
      FunctionResponseTypes: ["ReportBatchItemFailures"],
      MaximumBatchingWindowInSeconds: 1,
    });
  });

  test.each(STAGES)("uses CLI credentials synthesizer for %s", (stage) => {
    const stacks = createStacks(stage);

    expect(stacks.data.synthesizer).toBeInstanceOf(cdk.CliCredentialsStackSynthesizer);
    expect(stacks.compute.synthesizer).toBeInstanceOf(cdk.CliCredentialsStackSynthesizer);
    expect(stacks.api.synthesizer).toBeInstanceOf(cdk.CliCredentialsStackSynthesizer);
    if (stacks.observability) {
      expect(stacks.observability.synthesizer).toBeInstanceOf(cdk.CliCredentialsStackSynthesizer);
    }
  });

  test("real stages configure custom API domains through CloudFront", () => {
    const prodTemplates = synthesize("prod");
    const devTemplates = synthesize("dev");
    const ephemeralTemplates = synthesize("ephemeral");

    prodTemplates.api.hasResourceProperties("AWS::ApiGatewayV2::Api", {
      DisableExecuteApiEndpoint: true,
    });
    prodTemplates.api.hasResourceProperties("AWS::ApiGatewayV2::DomainName", {
      DomainName: "api.aura-historia.com",
      DomainNameConfigurations: Match.arrayWith([
        Match.objectLike({
          CertificateArn: "{{resolve:ssm:/certificates/prod/api-regional-certificate-arn}}",
          EndpointType: "REGIONAL",
          SecurityPolicy: "TLS_1_2",
        }),
      ]),
      RoutingMode: "API_MAPPING_ONLY",
    });
    prodTemplates.api.hasResourceProperties("AWS::CloudFront::Distribution", {
      DistributionConfig: Match.objectLike({
        Aliases: ["api.aura-historia.com"],
        Comment: "prod api",
        DefaultCacheBehavior: Match.objectLike({
          CachePolicyId: "4135ea2d-6df8-44a3-9df3-4b5a84be39ad",
          OriginRequestPolicyId: "216adef6-5c7f-47e4-b989-5492eafa07d3",
          ViewerProtocolPolicy: "redirect-to-https",
        }),
        CacheBehaviors: Match.arrayWith([
          Match.objectLike({
            CachePolicyId: "4cc15a8a-d715-48a4-82b8-cc0b614638fe",
            FunctionAssociations: Match.arrayWith([
              Match.objectLike({ EventType: "viewer-request" }),
            ]),
            PathPattern: "/api/*",
          }),
        ]),
        ViewerCertificate: Match.objectLike({
          AcmCertificateArn: "{{resolve:ssm:/certificates/prod/api-cloudfront-certificate-arn}}",
          MinimumProtocolVersion: "TLSv1.2_2021",
          SslSupportMethod: "sni-only",
        }),
        Origins: Match.arrayWith([
          Match.objectLike({
            DomainName: {
              "Fn::GetAtt": [Match.stringLikeRegexp("HttpApiApiDomainName"), "RegionalDomainName"],
            },
          }),
        ]),
        PriceClass: "PriceClass_100",
        WebACLId: {
          "Fn::GetAtt": [Match.stringLikeRegexp("HttpApiApiWebAcl"), "WebAclArn"],
        },
      }),
    });
    prodTemplates.api.hasResourceProperties("AWS::CloudFormation::CustomResource", {
      Name: "application-prod-api-cloudfront-web-acl",
      Region: "us-east-1",
      Rules: Match.arrayWith([
        Match.objectLike({ Name: "AWS-AWSManagedRulesAmazonIpReputationList" }),
        Match.objectLike({ Name: "AWS-AWSManagedRulesCommonRuleSet" }),
        Match.objectLike({ Name: "AWS-AWSManagedRulesKnownBadInputsRuleSet" }),
      ]),
      Scope: "CLOUDFRONT",
    });

    devTemplates.api.hasResourceProperties("AWS::ApiGatewayV2::DomainName", {
      DomainName: "api.dev.aura-historia.com",
    });
    devTemplates.api.hasResourceProperties("AWS::CloudFront::Distribution", {
      DistributionConfig: Match.objectLike({
        Aliases: ["*.dev.aura-historia.com"],
        Comment: "dev api",
      }),
    });
    expect(resourceCount(ephemeralTemplates, "AWS::CloudFront::Distribution")).toBe(0);
    expect(resourceCount(ephemeralTemplates, "AWS::ApiGatewayV2::DomainName")).toBe(0);
  });

  test("prod enables production safeguards", () => {
    const templates = synthesize("prod");

    expect(resourceCount(templates, "AWS::OpenSearchService::Domain")).toBe(0);
    expect(resourceCount(templates, "AWS::SNS::Topic")).toBe(1);
    expect(resourcePropertiesCount(templates, "AWS::CloudWatch::Alarm", {})).toBe(49);
    templates.api.hasResourceProperties("AWS::ApiGatewayV2::Stage", {
      DefaultRouteSettings: Match.objectLike({
        DetailedMetricsEnabled: true,
        ThrottlingBurstLimit: 5000,
        ThrottlingRateLimit: 2000,
      }),
      AccessLogSettings: Match.objectLike({
        Format: Match.stringLikeRegexp("requestId"),
      }),
    });
    expect(templates.observability?.toJSON().Outputs?.AlarmNotificationTopicArn).toBeDefined();
  });

  test("all prod Lambdas have error alarms", () => {
    const templates = synthesize("prod");

    expect(lambdaMetricAlarmFunctionNames(templates.observability!.toJSON(), "Errors")).toEqual(
      lambdaNames(templates.compute.toJSON()),
    );
  });

  test("all prod API Lambdas have throttle alarms", () => {
    const templates = synthesize("prod");

    expect(lambdaMetricAlarmFunctionNames(templates.observability!.toJSON(), "Throttles")).toEqual(
      new Set([
        "newsletter-api-prod",
        "notification-api-prod",
        "oauth-api-prod",
        "partner-shop-application-api-prod",
        "product-api-prod",
        "product-api-partner-prod",
        "product-watchlist-api-prod",
        "search-filter-api-prod",
        "shop-api-prod",
        "stripe-api-prod",
        "user-api-prod",
        "webhook-api-prod",
      ]),
    );
  });

  test("dev omits production-only resources but keeps scheduled fx-rate sync", () => {
    const templates = synthesize("dev");

    expect(resourceCount(templates, "AWS::OpenSearchService::Domain")).toBe(0);
    expect(resourceCount(templates, "AWS::SNS::Topic")).toBe(0);
    expect(resourceCount(templates, "AWS::CloudWatch::Alarm")).toBe(0);
    hasResourceProperties(templates, "AWS::Events::Rule", {
      ScheduleExpression: "cron(0 6,18 * * ? *)",
    });
    const resources = templates.compute.toJSON().Resources ?? {};
    expect(
      Object.values(resources).some(
        (resource: any) =>
          resource.Type === "AWS::Lambda::Function" &&
          JSON.stringify(resource.Properties?.Code?.S3Key).includes("fxrate-lambda"),
      ),
    ).toBe(true);
    templates.compute.hasResourceProperties("AWS::CloudFormation::CustomResource", {
      SourceEventId: "deployment:fxrate:initial:dev:v1",
    });
  });

  test("dev schedules periodic search-filter matching on Fargate", () => {
    const templates = synthesize("dev");

    expect(resourceCount(templates, "AWS::ECS::TaskDefinition")).toBe(1);
    hasResourceProperties(templates, "AWS::Events::Rule", {
      ScheduleExpression: "cron(0 15 * * ? *)",
    });
    hasResourceProperties(templates, "AWS::ECS::TaskDefinition", {
      Cpu: "1024",
      Family: "search-filter-periodic-match-dev",
      Memory: "2048",
      ContainerDefinitions: Match.arrayWith([
        Match.objectLike({
          Name: "search-filter-periodic-match",
          Environment: Match.arrayWith([
            Match.objectLike({ Name: "GEMINI_MODEL", Value: "gemini-3.1-flash-lite" }),
            Match.objectLike({ Name: "PERIODIC_MATCH_LLM_CONCURRENCY", Value: "50" }),
          ]),
        }),
      ]),
    });
  });

  test("ephemeral creates LocalStack-only resources", () => {
    const templates = synthesize("ephemeral");

    expect(resourceCount(templates, "AWS::OpenSearchService::Domain")).toBe(1);
    expect(resourceCount(templates, "AWS::Events::EventBus")).toBe(3);
    expect(resourceCount(templates, "AWS::SNS::Topic")).toBe(0);
    expect(resourceCount(templates, "AWS::CloudWatch::Alarm")).toBe(0);
    expect(resourcePropertiesCount(templates, "AWS::Lambda::Function", {
      FunctionName: Match.stringLikeRegexp("fxrate-lambda"),
    })).toBe(0);
    expect(resourcePropertiesCount(templates, "AWS::CloudFormation::CustomResource", {
      SourceEventId: Match.anyValue(),
    })).toBe(0);
    expect(resourceCount(templates, "AWS::ECS::TaskDefinition")).toBe(0);
    templates.data.hasResourceProperties("AWS::OpenSearchService::Domain", {
      DomainName: "test-domain",
      DomainEndpointOptions: {
        CustomEndpointEnabled: true,
        CustomEndpoint: "http://localhost:4566/test-domain",
      },
    });
  });

  test("lambda artifacts are selected by stage and deploy commit SHA", () => {
    const templates = synthesize("dev");

    templates.compute.hasResourceProperties("AWS::Lambda::Function", {
      Code: {
        S3Bucket: ARTIFACT_BUCKET_NAME,
        S3Key: {
          "Fn::Join": [
            "",
            Match.arrayWith([
              "product-api-dev-",
              { Ref: "CommitSHA" },
              ".zip",
            ]),
          ],
        },
      },
    });
  });

  test("critical API routes are configured with expected auth", () => {
    const templates = synthesize("ephemeral");

    templates.api.hasResourceProperties("AWS::ApiGatewayV2::Route", {
      RouteKey: "GET /api/v1/products",
      AuthorizationType: "NONE",
    });
    templates.api.hasResourceProperties("AWS::ApiGatewayV2::Route", {
      RouteKey: "GET /api/v1/me/account",
      AuthorizationType: "JWT",
      AuthorizerId: Match.anyValue(),
    });
    templates.api.hasResourceProperties("AWS::ApiGatewayV2::Route", {
      RouteKey: "POST /api/v1/webhooks/woocommerce/{shopId}",
      AuthorizationType: "NONE",
    });
  });

  test("prod CORS is restricted while non-prod remains permissive", () => {
    expect(apiCorsOrigins(synthesize("prod").api.toJSON())).toEqual([
      "https://aura-historia.com",
      "https://admin.shopify.com",
      "https://partners.shopify.com",
      "https://shopify.com",
      "https://*.myshopify.com",
    ]);
    expect(apiCorsOrigins(synthesize("dev").api.toJSON())).toEqual(["*"]);
    expect(apiCorsOrigins(synthesize("ephemeral").api.toJSON())).toEqual(["*"]);
  });

  test("Cognito uses public-client OAuth URLs and HTML verification email", () => {
    const prodTemplates = synthesize("prod");
    const devTemplates = synthesize("dev");

    prodTemplates.compute.hasResourceProperties("AWS::Cognito::UserPoolClient", {
      CallbackURLs: ["https://aura-historia.com/"],
      ExplicitAuthFlows: Match.arrayWith(["ALLOW_USER_PASSWORD_AUTH", "ALLOW_USER_SRP_AUTH"]),
      LogoutURLs: ["https://aura-historia.com/"],
    });
    devTemplates.compute.hasResourceProperties("AWS::Cognito::UserPoolClient", {
      CallbackURLs: ["http://localhost:3000", "https://stage.aura-historia.com/"],
      LogoutURLs: ["http://localhost:3000", "https://stage.aura-historia.com/"],
    });
    prodTemplates.compute.hasResourceProperties("AWS::Cognito::UserPool", {
      AccountRecoverySetting: {
        RecoveryMechanisms: [
          {
            Name: "verified_email",
            Priority: 1,
          },
        ],
      },
      EmailConfiguration: {
        ConfigurationSet: "my-first-configuration-set",
        EmailSendingAccount: "DEVELOPER",
        From: "Aura Historia <auth@notify.aura-historia.com>",
        ReplyToEmailAddress: "contact@aura-historia.com",
        SourceArn: {
          "Fn::Sub": [
            "arn:aws:ses:${AWS::Region}:${AWS::AccountId}:identity/${IdentityDomain}",
            { IdentityDomain: "notify.aura-historia.com" },
          ],
        },
      },
      UserPoolAddOns: {
        AdvancedSecurityMode: "ENFORCED",
      },
      UserPoolTier: "PLUS",
      VerificationMessageTemplate: Match.objectLike({
        EmailSubject: "Verify your email",
        EmailMessage: Match.stringLikeRegexp("<p class=\\\"greeting\\\">Verify your email</p>"),
      }),
    });
  });

  test.each(STAGES)("Postgres connection settings are explicit for %s", (stage) => {
    const templates = synthesize(stage);
    const json = JSON.stringify(templateList(templates).map((template) => template.toJSON()));

    if (stage === "ephemeral") {
      expect(json).toContain("host.docker.internal");
      expect(json).toContain("POSTGRES_MAX_CONNECTIONS");
      expect(json).toContain("postgres");
      return;
    }

    expect(json).toContain(`{{resolve:ssm:/postgres/${stage}/host}}`);
    expect(json).toContain(`{{resolve:ssm:/postgres/${stage}/port}}`);
    expect(json).toContain(`{{resolve:ssm:/postgres/${stage}/database}}`);
    expect(json).toContain(`{{resolve:ssm:/postgres/${stage}/username}}`);
    expect(json).toContain(`{{resolve:ssm:/secrets/${stage}/postgres-password}}`);
  });

  test("real stages resolve external integration settings from SSM", () => {
    const prodJson = JSON.stringify(templateList(synthesize("prod")).map((template) => template.toJSON()));
    const devJson = JSON.stringify(templateList(synthesize("dev")).map((template) => template.toJSON()));
    const ephemeralJson = JSON.stringify(templateList(synthesize("ephemeral")).map((template) => template.toJSON()));

    expect(prodJson).toContain("{{resolve:ssm:/opensearch/prod/endpoint-url}}");
    expect(prodJson).toContain("{{resolve:ssm:/eventbridge/prod/stripe-event-bus-name}}");
    expect(prodJson).toContain("{{resolve:ssm:/eventbridge/prod/shopify-event-bus-name}}");
    expect(prodJson).toContain("{{resolve:ssm:/stripe/prod/pro-monthly-price-id}}");
    expect(prodJson).toContain("{{resolve:ssm:/stripe/prod/ultimate-yearly-price-id}}");
    expect(prodJson).toContain("{{resolve:ssm:/certificates/prod/api-regional-certificate-arn}}");
    expect(prodJson).toContain("{{resolve:ssm:/certificates/prod/api-cloudfront-certificate-arn}}");
    expect(prodJson).not.toContain("/cloudfront/prod/api-web-acl-arn");

    expect(devJson).toContain("{{resolve:ssm:/opensearch/dev/endpoint-url}}");
    expect(devJson).toContain("{{resolve:ssm:/eventbridge/dev/stripe-event-bus-name}}");
    expect(devJson).toContain("{{resolve:ssm:/stripe/dev/pro-product-id}}");
    expect(devJson).toContain("{{resolve:ssm:/certificates/dev/api-regional-certificate-arn}}");
    expect(devJson).toContain("{{resolve:ssm:/certificates/dev/api-cloudfront-certificate-arn}}");
    expect(devJson).not.toContain("/cloudfront/dev/api-web-acl-arn");

    expect(ephemeralJson).not.toContain("/opensearch/ephemeral/endpoint-url");
    expect(ephemeralJson).toContain("stripe-event-bus-ephemeral");
    expect(ephemeralJson).toContain("price_pro_monthly_mock");
  });

  test("partner application callback tokens use wildcard authorization", () => {
    const templates = synthesize("ephemeral");

    templates.compute.hasResourceProperties("AWS::IAM::Policy", {
      PolicyDocument: {
        Statement: Match.arrayWith([
          Match.objectLike({
            Action: Match.arrayWith([
              "states:SendTaskSuccess",
              "states:SendTaskFailure",
              "states:SendTaskHeartbeat",
            ]),
            Resource: "*",
          }),
        ]),
      },
    });
    templates.compute.hasResourceProperties("AWS::IAM::Policy", {
      PolicyDocument: {
        Statement: Match.arrayWith([
          Match.objectLike({
            Action: Match.arrayWith(["states:StartExecution", "states:DescribeExecution"]),
            Resource: {
              Ref: Match.stringLikeRegexp("PartnerShopApplicationWorkflow.*StateMachine"),
            },
          }),
        ]),
      },
    });
  });

  test("queues are created with dead-letter queues", () => {
    const templates = synthesize("ephemeral");

    expect(resourcePropertiesCount(templates, "AWS::SQS::Queue", {
      RedrivePolicy: Match.anyValue(),
    })).toBe(13);
    templates.data.hasResourceProperties("AWS::SQS::Queue", {
      RedrivePolicy: Match.objectLike({
        maxReceiveCount: 5,
      }),
    });
  });
});

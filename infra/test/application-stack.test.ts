import * as cdk from "aws-cdk-lib";
import { Match, Template } from "aws-cdk-lib/assertions";
import { ApplicationStack } from "../src/application-stack";
import { ARTIFACT_BUCKET_NAME, STAGES, type StageName } from "../src/config";

type TemplateJson = ReturnType<Template["toJSON"]>;
type CfnResource = {
  readonly Type?: string;
  readonly Properties?: Record<string, any>;
};

function synthesize(stage: StageName): Template {
  const app = new cdk.App({
    analyticsReporting: false,
  });

  const stack = new ApplicationStack(app, `application-${stage}`, {
    stage,
    stackName: `application-${stage}`,
  });

  return Template.fromStack(stack);
}

function resourcesOfType(json: TemplateJson, type: string): Array<[string, CfnResource]> {
  return Object.entries((json.Resources ?? {}) as Record<string, CfnResource>).filter(([, resource]) => resource.Type === type);
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

function functionNameByLogicalId(json: TemplateJson): Map<string, string> {
  return new Map(
    resourcesOfType(json, "AWS::Lambda::Function").map(([logicalId, resource]) => [
      logicalId,
      resource.Properties?.FunctionName as string,
    ]),
  );
}

function lambdaMetricAlarmFunctionNames(json: TemplateJson, metricName: string): Set<string> {
  const namesByLogicalId = functionNameByLogicalId(json);
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
    } else if (value?.Ref && namesByLogicalId.has(value.Ref)) {
      names.add(namesByLogicalId.get(value.Ref)!);
    }
  }

  return names;
}

describe("ApplicationStack", () => {
  test.each(STAGES)("synthesizes the %s stack contract", (stage) => {
    const template = synthesize(stage);

    template.resourceCountIs("AWS::DynamoDB::Table", 1);
    template.resourceCountIs("AWS::Cognito::UserPool", 1);
    template.resourceCountIs("AWS::Cognito::UserPoolClient", 1);
    template.resourceCountIs("AWS::ApiGatewayV2::Api", 1);
    template.resourceCountIs("AWS::ApiGatewayV2::Route", 71);
    template.resourceCountIs("AWS::ApiGatewayV2::Integration", 12);
    template.resourceCountIs("AWS::SQS::Queue", 24);
    template.resourceCountIs("AWS::Lambda::EventSourceMapping", 12);
    template.resourceCountIs("AWS::StepFunctions::StateMachine", 1);
    template.resourceCountIs("AWS::Pipes::Pipe", 1);

    const json = template.toJSON();
    expect(Object.keys(json.Parameters ?? {})).toEqual(["CommitSHA"]);
    expect(json.Resources?.CDKMetadata).toBeUndefined();
    expect(json.Outputs?.ApiGatewayEndpointUrl).toBeDefined();
    expect(json.Outputs?.DynamodbTable1Name).toBeDefined();
    expect(json.Outputs?.CognitoUserPoolId).toBeDefined();
    expect(json.Outputs?.CognitoUserPoolClientPublicId).toBeDefined();
    expect(json.Outputs?.CognitoUserPoolClientAdminId).toBeUndefined();
    expect(json.Outputs?.NotificationSendQueueUrl).toBeDefined();
    expect(json.Outputs?.StripeEventBusName).toBeDefined();
    expect(json.Outputs?.ShopifyEventBusName).toBeDefined();
  });

  test("prod enables production safeguards", () => {
    const template = synthesize("prod");

    template.resourceCountIs("AWS::OpenSearchService::Domain", 0);
    template.resourceCountIs("AWS::SNS::Topic", 1);
    template.resourcePropertiesCountIs("AWS::CloudWatch::Alarm", {}, 47);
    template.hasResourceProperties("AWS::ApiGatewayV2::Stage", {
      DefaultRouteSettings: Match.objectLike({
        DetailedMetricsEnabled: true,
        ThrottlingBurstLimit: 5000,
        ThrottlingRateLimit: 2000,
      }),
      AccessLogSettings: Match.objectLike({
        Format: Match.stringLikeRegexp("requestId"),
      }),
    });
    template.hasOutput("AlarmNotificationTopicArn", {});
  });

  test("all prod Lambdas have error alarms", () => {
    const json = synthesize("prod").toJSON();

    expect(lambdaMetricAlarmFunctionNames(json, "Errors")).toEqual(lambdaNames(json));
  });

  test("all prod API Lambdas have throttle alarms", () => {
    const json = synthesize("prod").toJSON();

    expect(lambdaMetricAlarmFunctionNames(json, "Throttles")).toEqual(
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
    const template = synthesize("dev");

    template.resourceCountIs("AWS::OpenSearchService::Domain", 0);
    template.resourceCountIs("AWS::SNS::Topic", 0);
    template.resourceCountIs("AWS::CloudWatch::Alarm", 0);
    template.hasResourceProperties("AWS::Events::Rule", {
      ScheduleExpression: "cron(0 6,18 * * ? *)",
    });
    const resources = template.toJSON().Resources ?? {};
    expect(
      Object.values(resources).some(
        (resource: any) =>
          resource.Type === "AWS::Lambda::Function" &&
          JSON.stringify(resource.Properties?.Code?.S3Key).includes("fxrate-lambda"),
      ),
    ).toBe(true);
  });

  test("ephemeral creates LocalStack-only resources", () => {
    const template = synthesize("ephemeral");

    template.resourceCountIs("AWS::OpenSearchService::Domain", 1);
    template.resourceCountIs("AWS::Events::EventBus", 3);
    template.resourceCountIs("AWS::SNS::Topic", 0);
    template.resourceCountIs("AWS::CloudWatch::Alarm", 0);
    template.resourcePropertiesCountIs(
      "AWS::Lambda::Function",
      {
        FunctionName: Match.stringLikeRegexp("fxrate-lambda"),
      },
      0,
    );
    template.hasResourceProperties("AWS::OpenSearchService::Domain", {
      DomainName: "test-domain",
      DomainEndpointOptions: {
        CustomEndpointEnabled: true,
        CustomEndpoint: "http://localhost:4566/test-domain",
      },
    });
  });

  test("lambda artifacts are selected by stage and deploy commit SHA", () => {
    const template = synthesize("dev");

    template.hasResourceProperties("AWS::Lambda::Function", {
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
    const template = synthesize("ephemeral");

    template.hasResourceProperties("AWS::ApiGatewayV2::Route", {
      RouteKey: "GET /api/v1/products",
      AuthorizationType: "NONE",
    });
    template.hasResourceProperties("AWS::ApiGatewayV2::Route", {
      RouteKey: "GET /api/v1/me/account",
      AuthorizationType: "JWT",
      AuthorizerId: Match.anyValue(),
    });
    template.hasResourceProperties("AWS::ApiGatewayV2::Route", {
      RouteKey: "POST /api/v1/webhooks/woocommerce/{shopId}",
      AuthorizationType: "NONE",
    });
  });

  test("prod CORS is restricted while non-prod remains permissive", () => {
    expect(apiCorsOrigins(synthesize("prod").toJSON())).toEqual([
      "https://aura-historia.com",
      "https://admin.shopify.com",
      "https://partners.shopify.com",
      "https://shopify.com",
      "https://*.myshopify.com",
    ]);
    expect(apiCorsOrigins(synthesize("dev").toJSON())).toEqual(["*"]);
    expect(apiCorsOrigins(synthesize("ephemeral").toJSON())).toEqual(["*"]);
  });

  test("Cognito uses public-client OAuth URLs and HTML verification email", () => {
    const prodTemplate = synthesize("prod");
    const devTemplate = synthesize("dev");

    prodTemplate.hasResourceProperties("AWS::Cognito::UserPoolClient", {
      CallbackURLs: ["https://aura-historia.com/"],
      ExplicitAuthFlows: Match.arrayWith(["ALLOW_USER_PASSWORD_AUTH", "ALLOW_USER_SRP_AUTH"]),
      LogoutURLs: ["https://aura-historia.com/"],
    });
    devTemplate.hasResourceProperties("AWS::Cognito::UserPoolClient", {
      CallbackURLs: ["http://localhost:3000", "https://stage.aura-historia.com/"],
      LogoutURLs: ["http://localhost:3000", "https://stage.aura-historia.com/"],
    });
    prodTemplate.hasResourceProperties("AWS::Cognito::UserPool", {
      VerificationMessageTemplate: Match.objectLike({
        EmailSubject: "Verify your email",
        EmailMessage: Match.stringLikeRegexp("<p class=\\\"greeting\\\">Verify your email</p>"),
      }),
    });
  });

  test("real stages resolve external integration settings from SSM", () => {
    const prodJson = JSON.stringify(synthesize("prod").toJSON());
    const devJson = JSON.stringify(synthesize("dev").toJSON());
    const ephemeralJson = JSON.stringify(synthesize("ephemeral").toJSON());

    expect(prodJson).toContain("{{resolve:ssm:/opensearch/prod/endpoint-url}}");
    expect(prodJson).toContain("{{resolve:ssm:/eventbridge/prod/stripe-event-bus-name}}");
    expect(prodJson).toContain("{{resolve:ssm:/eventbridge/prod/shopify-event-bus-name}}");
    expect(prodJson).toContain("{{resolve:ssm:/stripe/prod/pro-monthly-price-id}}");
    expect(prodJson).toContain("{{resolve:ssm:/stripe/prod/ultimate-yearly-price-id}}");

    expect(devJson).toContain("{{resolve:ssm:/opensearch/dev/endpoint-url}}");
    expect(devJson).toContain("{{resolve:ssm:/eventbridge/dev/stripe-event-bus-name}}");
    expect(devJson).toContain("{{resolve:ssm:/stripe/dev/pro-product-id}}");

    expect(ephemeralJson).not.toContain("/opensearch/ephemeral/endpoint-url");
    expect(ephemeralJson).toContain("stripe-event-bus-ephemeral");
    expect(ephemeralJson).toContain("price_pro_monthly_mock");
  });

  test("partner application callback tokens use wildcard authorization", () => {
    const template = synthesize("ephemeral");

    template.hasResourceProperties("AWS::IAM::Policy", {
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
    template.hasResourceProperties("AWS::IAM::Policy", {
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
    const template = synthesize("ephemeral");

    template.resourcePropertiesCountIs(
      "AWS::SQS::Queue",
      {
        RedrivePolicy: Match.anyValue(),
      },
      12,
    );
    template.hasResourceProperties("AWS::SQS::Queue", {
      RedrivePolicy: Match.objectLike({
        maxReceiveCount: 5,
      }),
    });
  });
});

import * as cdk from "aws-cdk-lib";
import { Match, Template } from "aws-cdk-lib/assertions";
import { ApplicationStack } from "../src/application-stack";
import { STAGES, type StageName } from "../src/config";

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

describe("ApplicationStack", () => {
  test.each(STAGES)("synthesizes the %s stack contract", (stage) => {
    const template = synthesize(stage);

    template.resourceCountIs("AWS::DynamoDB::Table", 1);
    template.resourceCountIs("AWS::Cognito::UserPool", 1);
    template.resourceCountIs("AWS::Cognito::UserPoolClient", 2);
    template.resourceCountIs("AWS::ApiGatewayV2::Api", 1);
    template.resourceCountIs("AWS::ApiGatewayV2::Route", 71);
    template.resourceCountIs("AWS::ApiGatewayV2::Integration", 12);
    template.resourceCountIs("AWS::SQS::Queue", 24);
    template.resourceCountIs("AWS::Lambda::EventSourceMapping", 12);
    template.resourceCountIs("AWS::StepFunctions::StateMachine", 1);
    template.resourceCountIs("AWS::Pipes::Pipe", 1);

    const json = template.toJSON();
    expect(json.Resources?.CDKMetadata).toBeUndefined();
    expect(json.Outputs?.ApiGatewayEndpointUrl).toBeDefined();
    expect(json.Outputs?.DynamodbTable1Name).toBeDefined();
    expect(json.Outputs?.CognitoUserPoolId).toBeDefined();
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
    template.resourcePropertiesCountIs("AWS::Lambda::Function", {
      FunctionName: Match.stringLikeRegexp("fxrate-lambda"),
    }, 0);
    template.hasResourceProperties("AWS::OpenSearchService::Domain", {
      DomainName: "test-domain",
      DomainEndpointOptions: {
        CustomEndpointEnabled: true,
        CustomEndpoint: "http://localhost:4566/test-domain",
      },
    });
  });

  test("lambda artifacts are selected by stage name and deploy commit SHA", () => {
    const template = synthesize("dev");

    template.hasResourceProperties("AWS::Lambda::Function", {
      Code: {
        S3Bucket: { Ref: "ArtifactBucket" },
        S3Key: {
          "Fn::Join": [
            "",
            Match.arrayWith([
              "product-api-",
              { Ref: "StageName" },
              "-",
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

    template.resourcePropertiesCountIs("AWS::SQS::Queue", {
      RedrivePolicy: Match.anyValue(),
    }, 12);
    template.hasResourceProperties("AWS::SQS::Queue", {
      RedrivePolicy: Match.objectLike({
        maxReceiveCount: 5,
      }),
    });
  });
});

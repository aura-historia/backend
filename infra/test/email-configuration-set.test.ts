import * as cdk from "aws-cdk-lib";
import { Template } from "aws-cdk-lib/assertions";
import * as fs from "node:fs";
import * as path from "node:path";
import { createApplicationStacks } from "../src/application-stack";
import { stageConfig, STAGES } from "../src/config";

function sourceContents(directory: string): string[] {
  return fs.readdirSync(directory, { withFileTypes: true }).flatMap((entry) => {
    const entryPath = path.join(directory, entry.name);
    return entry.isDirectory() ? sourceContents(entryPath) : [fs.readFileSync(entryPath, "utf8")];
  });
}

test("infrastructure source does not refer to the legacy shared configuration set", () => {
  expect(sourceContents(path.join(__dirname, "../src")).join("\n")).not.toContain("my-first");
});

describe.each(STAGES)("%s managed SES configuration set", (stage) => {
  const stacks = createApplicationStacks(new cdk.App({ analyticsReporting: false }), { stage });
  const compute = Template.fromStack(stacks.compute);
  const [[setId, set]] = Object.entries(compute.findResources("AWS::SES::ConfigurationSet"));
  const [[destinationId, destination]] = Object.entries(
    compute.findResources("AWS::SES::ConfigurationSetEventDestination"),
  );
  const setRef = { Ref: setId };
  const otherStage = stage === "dev" ? "prod" : "dev";

  test("owns exactly one native retained set in compute and leaves the shared identity unmanaged", () => {
    const config = stageConfig(stage);
    expect(config.notificationEmail.configurationSet).toBe(`aura-historia-${stage}-email`);
    expect(config.cognitoEmail?.configurationSet).toBe(config.notificationEmail.configurationSet);
    compute.resourceCountIs("AWS::SES::ConfigurationSet", 1);
    compute.resourceCountIs("AWS::SES::ConfigurationSetEventDestination", 1);
    expect(set.Properties).toEqual({
      Name: `aura-historia-${stage}-email`,
      ReputationOptions: { ReputationMetricsEnabled: true },
    });
    expect(set.DeletionPolicy).toBe("Retain");
    expect(set.UpdateReplacePolicy).toBe("Retain");

    for (const stack of Object.values(stacks)) {
      if (!stack) continue;
      const template = Template.fromStack(stack);
      const resources = Object.values(template.toJSON().Resources) as Array<{ Type: string; Properties?: unknown }>;
      const sesResources = resources.filter((resource) => resource.Type.startsWith("AWS::SES::"));
      expect(sesResources).toHaveLength(stack === stacks.compute ? 2 : 0);
      for (const resource of resources.filter((candidate) => candidate.Type.startsWith("Custom::"))) {
        expect(JSON.stringify(resource)).not.toMatch(/ses:|configuration.?set|email.?identity/i);
      }
      expect(JSON.stringify(template.toJSON())).not.toContain("my-first");
      expect(JSON.stringify(template.toJSON())).not.toContain(`aura-historia-${otherStage}-email`);
    }
  });

  test("publishes only the existing event types as CloudWatch metrics with a stage message-tag default", () => {
    expect(destination.Properties).toEqual({
      ConfigurationSetName: setRef,
      EventDestination: {
        Enabled: true,
        MatchingEventTypes: [
          "send", "delivery", "bounce", "complaint", "deliveryDelay", "renderingFailure",
          "open", "click", "subscription",
        ],
        CloudWatchDestination: {
          DimensionConfigurations: [{
            DimensionName: "stage",
            DimensionValueSource: "messageTag",
            DefaultDimensionValue: stage,
          }],
        },
      },
    });
  });

  test("Cognito consumes the managed reference after metrics without changing permanent identity resources", () => {
    const [[poolId, pool]] = Object.entries(compute.findResources("AWS::Cognito::UserPool"));
    expect(poolId).toBe("IdentityPrimaryUserPool6ED02BC6");
    expect(Object.keys(compute.findResources("AWS::Cognito::UserPoolClient")))
      .toEqual(["IdentityPrimaryUserPoolPrimaryUserPoolClientPublic80694516"]);
    expect(Object.keys(compute.findResources("AWS::Cognito::UserPoolDomain")))
      .toEqual(["IdentityPrimaryUserPoolPrimaryUserPoolDomainC98AD955"]);
    expect(Object.keys(compute.findResources("AWS::Cognito::UserPoolIdentityProvider")))
      .toEqual(["IdentityGoogleIdentityProviderB503749B", "IdentityFacebookIdentityProviderD75F6B2A"]);
    expect(pool.Properties.Schema).toEqual([
      { Name: "email", Required: true, Mutable: true },
      { Name: "given_name", Required: false, Mutable: true },
      { Name: "family_name", Required: false, Mutable: true },
      { Name: "locale", Required: false, Mutable: true },
      { Name: "marketing_consent", AttributeDataType: "String", Mutable: false },
    ]);
    expect(pool.Properties.EmailConfiguration).toEqual({
      ConfigurationSet: setRef,
      EmailSendingAccount: "DEVELOPER",
      From: "Aura Historia <auth@notify.aura-historia.com>",
      ReplyToEmailAddress: "contact@aura-historia.com",
      SourceArn: {
        "Fn::Sub": ["arn:aws:ses:${AWS::Region}:${AWS::AccountId}:identity/${IdentityDomain}", {
          IdentityDomain: "notify.aura-historia.com",
        }],
      },
    });
    expect(pool.DependsOn).toContain(destinationId);
  });

  test.each([
    ["aura-historia-api", "NEWSLETTER_CONFIRMATION_EMAIL_CONFIGURATION_SET"],
    ["notification-delivery-lambda", "NOTIFICATION_EMAIL_CONFIGURATION_SET"],
  ])("%s selects and can send only through its own managed set", (binaryName, environmentKey) => {
    const functions = Object.values(compute.findResources("AWS::Lambda::Function"));
    const sender = functions.find((resource) => resource.Properties.FunctionName === `${binaryName}-${stage}`)!;
    expect(sender.Properties.Environment.Variables[environmentKey]).toEqual(setRef);
    expect(sender.DependsOn).toContain(destinationId);

    const roleId = sender.Properties.Role["Fn::GetAtt"][0];
    const policies = Object.values(compute.findResources("AWS::IAM::Policy"))
      .filter((policy) => policy.Properties.Roles.some((role: { Ref: string }) => role.Ref === roleId));
    const sesStatements = policies.flatMap((policy) => policy.Properties.PolicyDocument.Statement)
      .filter((statement) => JSON.stringify(statement.Action).includes("ses:"));
    expect(sesStatements).toEqual([{
      Effect: "Allow",
      Action: "ses:SendEmail",
      Resource: [
        { "Fn::Join": ["", [
          "arn:", { Ref: "AWS::Partition" }, ":ses:", { Ref: "AWS::Region" }, ":",
          { Ref: "AWS::AccountId" }, ":identity/notify.aura-historia.com",
        ]] },
        { "Fn::Join": ["", [
          "arn:", { Ref: "AWS::Partition" }, ":ses:", { Ref: "AWS::Region" }, ":",
          { Ref: "AWS::AccountId" }, ":configuration-set/", setRef,
        ]] },
      ],
    }]);
  });
});

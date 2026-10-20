import * as cdk from "aws-cdk-lib";
import { Template, Match } from "aws-cdk-lib/assertions";
import { ReleaseOperationsStack } from "../src/release-operations-stack";

const template = () =>
  Template.fromStack(
    new ReleaseOperationsStack(new cdk.App(), "release-fixture", {
      env: { account: "123456789012", region: "eu-central-1" },
    }),
  );

test("release ownership survives runner loss and stack deletion with versioned conditional state", () => {
  const rendered = template();
  rendered.hasResource("AWS::S3::Bucket", {
    DeletionPolicy: "Retain",
    UpdateReplacePolicy: "Retain",
    Properties: {
      VersioningConfiguration: { Status: "Enabled" },
      PublicAccessBlockConfiguration: {
        BlockPublicAcls: true,
        BlockPublicPolicy: true,
        IgnorePublicAcls: true,
        RestrictPublicBuckets: true,
      },
    },
  });
  rendered.hasResourceProperties("AWS::S3::BucketPolicy", {
    PolicyDocument: {
      Statement: Match.arrayWith([
        Match.objectLike({
          Effect: "Deny",
          Action: "s3:PutObject",
          Condition: { Null: { "s3:if-match": "true", "s3:if-none-match": "true" } },
        }),
        Match.objectLike({ Effect: "Deny", Action: ["s3:DeleteObject", "s3:DeleteObjectVersion"] }),
      ]),
    },
  });
});

test("independent monitor alerts on overdue pauses and missing monitoring without mutating capture", () => {
  const rendered = template();
  rendered.resourceCountIs("AWS::CloudWatch::Alarm", 4);
  rendered.hasResourceProperties("AWS::Events::Rule", { ScheduleExpression: "rate(1 minute)" });
  for (const stage of ["dev", "prod"])
    for (const kind of ["overdue", "monitor"])
      rendered.hasResourceProperties("AWS::CloudWatch::Alarm", {
        AlarmName: `aura-historia-release-${stage}-${kind}`,
        Namespace: "AuraHistoria/Release",
        TreatMissingData: "breaching",
        AlarmActions: [Match.objectLike({ Ref: Match.anyValue() })],
      });
  const policies = JSON.stringify(rendered.findResources("AWS::IAM::Policy"));
  expect(policies).toContain("s3:GetObject");
  expect(policies).toContain("cloudwatch:PutMetricData");
  expect(policies).not.toMatch(/dms:|s3:PutObject|s3:DeleteObject|lambda:Update|sqs:|scheduler:/);
});

import * as cdk from "aws-cdk-lib";
import { Template } from "aws-cdk-lib/assertions";
import { ApplicationEphemeralStack, createApplicationStacks } from "../src/application-stack";

const REAL_STAGES = ["dev", "prod"] as const;

const ROUTER_QUEUES = {
  SEARCH_FILTER_PROJECTION: "search-filter-projection",
  SEARCH_FILTER_PERCOLATOR: "search-filter-percolator",
  SEARCH_FILTER_MATCH_NOTIFICATION: "search-filter-match-notification",
  WATCHLIST_NOTIFICATION: "watchlist-notification",
  PRODUCT_LISTING_CONTENT_ASSESSMENT: "product-content-assessment",
  PRODUCT_LISTING_TRANSLATION: "product-translation",
  PRODUCT_LISTING_EMBEDDING: "product-embedding",
  PRODUCT_LISTING_OPENSEARCH: "product-listing-opensearch",
  PRODUCT_LISTING_RAW_NORMALIZATION: "product-listing-normalization",
  NOTIFICATION_DELIVERY: "notification-delivery",
} as const;

const KINESIS_READ_ACTIONS = [
  "kinesis:DescribeStream",
  "kinesis:DescribeStreamSummary",
  "kinesis:GetRecords",
  "kinesis:GetShardIterator",
  "kinesis:ListShards",
];

type Resource = {
  readonly DeletionPolicy?: unknown;
  readonly Properties: Record<string, unknown>;
  readonly UpdateReplacePolicy?: unknown;
};

type PolicyStatement = {
  readonly Action: string | string[];
  readonly Effect: string;
  readonly Resource: unknown;
};

function stackTemplates(stage: (typeof REAL_STAGES)[number]) {
  const app = new cdk.App({ analyticsReporting: false });
  const stacks = createApplicationStacks(app, { stage });
  return {
    data: Template.fromStack(stacks.data),
    compute: Template.fromStack(stacks.compute),
  };
}

function namedResource(template: Template, type: string, property: string, value: string): [string, Resource] {
  const match = Object.entries(template.findResources(type) as Record<string, Resource>)
    .find(([, resource]) => resource.Properties[property] === value);
  if (!match) {
    throw new Error(`Missing ${type} with ${property}=${value}.`);
  }
  return match;
}

function routerPolicyStatements(template: Template, routerLogicalId: string): PolicyStatement[] {
  const router = (template.findResources("AWS::Lambda::Function") as Record<string, Resource>)[routerLogicalId];
  const role = router.Properties.Role as { "Fn::GetAtt": [string, string] };
  const [roleLogicalId] = role["Fn::GetAtt"];
  const policy = Object.values(template.findResources("AWS::IAM::Policy") as Record<string, Resource>)
    .find((resource) => JSON.stringify(resource.Properties.Roles).includes(roleLogicalId)
      && JSON.stringify(resource.Properties.PolicyDocument).includes("kinesis:GetRecords"));
  if (!policy) {
    throw new Error("Missing CDC router execution policy.");
  }
  return (policy.Properties.PolicyDocument as { Statement: PolicyStatement[] }).Statement;
}

describe.each(REAL_STAGES)("%s DMS CDC router", (stage) => {
  test("uses the worker binary convention without VPC, PostgreSQL, secret, or concurrency configuration", () => {
    const { compute } = stackTemplates(stage);
    const [routerLogicalId, router] = namedResource(
      compute,
      "AWS::Lambda::Function",
      "FunctionName",
      `cdc-router-lambda-${stage}`,
    );
    const environment = (router.Properties.Environment as { Variables: Record<string, unknown> }).Variables;

    expect(router.Properties).toMatchObject({
      Architectures: ["x86_64"],
      Code: {
        S3Bucket: "aura-historia-binary-artifacts-eu-central-1",
        S3Key: { "Fn::Join": ["", [`cdc-router-lambda-${stage}-`, { Ref: "CommitSHA" }, ".zip"]] },
      },
      FunctionName: `cdc-router-lambda-${stage}`,
      Handler: "lib.handler",
      MemorySize: 256,
      Runtime: "provided.al2023",
      Timeout: 30,
    });
    expect(Object.keys(environment).sort()).toEqual(
      Object.keys(ROUTER_QUEUES).map((scope) => `AURA_HISTORIA_ROUTER_QUEUE_URL_${scope}`).sort(),
    );
    for (const [scope, workerScope] of Object.entries(ROUTER_QUEUES)) {
      expect(JSON.stringify(environment[`AURA_HISTORIA_ROUTER_QUEUE_URL_${scope}`]))
        .toContain(`aura-worker-${workerScope}-${stage}`);
    }
    expect(router.Properties.VpcConfig).toBeUndefined();
    expect(router.Properties.Layers).toBeUndefined();
    expect(router.Properties.ReservedConcurrentExecutions).toBeUndefined();
    expect(JSON.stringify(router.Properties)).not.toContain("POSTGRES_");
    expect(JSON.stringify(router.Properties)).not.toContain("SECRET");
    expect(JSON.stringify(router.Properties)).not.toContain("DmsCdc");
    expect(routerLogicalId).toContain("CdcRouterLambda");
  });

  test("maps the real DMS Kinesis stream with bounded failure handling and default-disabled activation", () => {
    const { data, compute } = stackTemplates(stage);
    const [routerLogicalId] = namedResource(
      compute,
      "AWS::Lambda::Function",
      "FunctionName",
      `cdc-router-lambda-${stage}`,
    );
    const [archiveLogicalId] = namedResource(
      compute,
      "AWS::S3::Bucket",
      "BucketName",
      `aura-historia-cdc-router-failures-${stage}`,
    );
    const mappings = Object.values(compute.findResources("AWS::Lambda::EventSourceMapping") as Record<string, Resource>);
    const mapping = mappings.find((resource) => JSON.stringify(resource.Properties.FunctionName).includes(routerLogicalId));

    expect(Object.values(data.findResources("AWS::Kinesis::Stream"))).toHaveLength(1);
    expect(mapping).toBeDefined();
    expect(Object.keys(mapping!.Properties).sort()).toEqual([
      "BatchSize",
      "BisectBatchOnFunctionError",
      "DestinationConfig",
      "Enabled",
      "EventSourceArn",
      "FunctionName",
      "FunctionResponseTypes",
      "MaximumBatchingWindowInSeconds",
      "MaximumRecordAgeInSeconds",
      "MaximumRetryAttempts",
      "StartingPosition",
    ]);
    expect(mapping!.Properties).toMatchObject({
      BatchSize: 100,
      BisectBatchOnFunctionError: true,
      DestinationConfig: {
        OnFailure: {
          Destination: { "Fn::GetAtt": [archiveLogicalId, "Arn"] },
        },
      },
      Enabled: { "Fn::If": ["ProductListingOpenSearchConsumerActivation", true, false] },
      FunctionName: { "Fn::GetAtt": [routerLogicalId, "Arn"] },
      FunctionResponseTypes: ["ReportBatchItemFailures"],
      MaximumBatchingWindowInSeconds: 1,
      MaximumRecordAgeInSeconds: 3600,
      MaximumRetryAttempts: 3,
      StartingPosition: "TRIM_HORIZON",
    });
    expect(JSON.stringify(mapping!.Properties.EventSourceArn)).toContain("DmsCdcCdcStream");
    expect(mapping!.Properties.ProvisionedPollerConfig).toBeUndefined();
    expect(mapping!.Properties.ScalingConfig).toBeUndefined();
  });

  test("retains an encrypted, private failure archive and grants the router only its read, fan-out, and archive actions", () => {
    const { compute } = stackTemplates(stage);
    const [routerLogicalId] = namedResource(
      compute,
      "AWS::Lambda::Function",
      "FunctionName",
      `cdc-router-lambda-${stage}`,
    );
    const [archiveLogicalId, archive] = namedResource(
      compute,
      "AWS::S3::Bucket",
      "BucketName",
      `aura-historia-cdc-router-failures-${stage}`,
    );
    const statements = routerPolicyStatements(compute, routerLogicalId);
    const kinesis = statements.find((statement) => JSON.stringify(statement.Action).includes("kinesis:GetRecords"));
    const sourceQueues = statements.find((statement) => JSON.stringify(statement.Action).includes("sqs:SendMessage"));
    const deadLetterQueues = statements.find((statement) => statement.Action === "sqs:GetQueueAttributes");
    const archiveList = statements.find((statement) => statement.Action === "s3:ListBucket");
    const archivePut = statements.find((statement) => statement.Action === "s3:PutObject");
    const bucketPolicy = Object.values(compute.findResources("AWS::S3::BucketPolicy") as Record<string, Resource>)
      .find((resource) => JSON.stringify(resource.Properties.Bucket).includes(archiveLogicalId));

    expect(archive).toMatchObject({
      DeletionPolicy: "Retain",
      Properties: {
        BucketEncryption: {
          ServerSideEncryptionConfiguration: [{
            ServerSideEncryptionByDefault: { SSEAlgorithm: "AES256" },
          }],
        },
        BucketName: `aura-historia-cdc-router-failures-${stage}`,
        LifecycleConfiguration: {
          Rules: [{
            ExpirationInDays: 90,
            Status: "Enabled",
          }],
        },
        PublicAccessBlockConfiguration: {
          BlockPublicAcls: true,
          BlockPublicPolicy: true,
          IgnorePublicAcls: true,
          RestrictPublicBuckets: true,
        },
      },
      UpdateReplacePolicy: "Retain",
    });
    expect(bucketPolicy).toMatchObject({
      Properties: {
        PolicyDocument: {
          Statement: [expect.objectContaining({
            Action: "s3:*",
            Condition: { Bool: { "aws:SecureTransport": "false" } },
            Effect: "Deny",
            Principal: { AWS: "*" },
          })],
        },
      },
    });

    expect(statements).toHaveLength(5);
    expect(kinesis).toMatchObject({
      Action: KINESIS_READ_ACTIONS,
      Effect: "Allow",
    });
    expect(kinesis?.Resource).toEqual(expect.anything());
    expect(JSON.stringify(kinesis?.Resource)).toContain("DmsCdcCdcStream");
    expect(sourceQueues).toMatchObject({
      Action: ["sqs:SendMessage", "sqs:GetQueueAttributes"],
      Effect: "Allow",
    });
    expect(deadLetterQueues).toMatchObject({
      Action: "sqs:GetQueueAttributes",
      Effect: "Allow",
    });
    const sourceQueueArns = sourceQueues?.Resource as unknown[];
    const deadLetterQueueArns = deadLetterQueues?.Resource as unknown[];
    expect(sourceQueueArns).toHaveLength(10);
    expect(deadLetterQueueArns).toHaveLength(10);
    for (const workerScope of Object.values(ROUTER_QUEUES)) {
      expect(JSON.stringify(sourceQueueArns)).toContain(`aura-worker-${workerScope}-${stage}`);
      expect(JSON.stringify(deadLetterQueueArns)).toContain(`aura-worker-${workerScope}-dlq-${stage}`);
    }
    expect(archiveList).toEqual({
      Action: "s3:ListBucket",
      Effect: "Allow",
      Resource: { "Fn::GetAtt": [archiveLogicalId, "Arn"] },
    });
    expect(archivePut).toMatchObject({
      Action: "s3:PutObject",
      Effect: "Allow",
    });
    expect(JSON.stringify(archivePut?.Resource)).toContain(archiveLogicalId);
    expect(JSON.stringify(archivePut?.Resource)).toContain("/*");

    const allActions = statements.flatMap((statement) => Array.isArray(statement.Action) ? statement.Action : [statement.Action]);
    expect(allActions.sort()).toEqual([
      ...KINESIS_READ_ACTIONS,
      "s3:ListBucket",
      "s3:PutObject",
      "sqs:GetQueueAttributes",
      "sqs:GetQueueAttributes",
      "sqs:SendMessage",
    ].sort());
    expect(JSON.stringify(statements)).not.toContain("kinesis:Put");
    expect(JSON.stringify(statements)).not.toContain("secretsmanager:");
    expect(JSON.stringify(statements)).not.toContain("sqs:ReceiveMessage");
  });
});

test("ephemeral does not construct the DMS router, failure archive, or Kinesis mapping", () => {
  const app = new cdk.App({ analyticsReporting: false });
  const template = Template.fromStack(new ApplicationEphemeralStack(app, "application-ephemeral", { stage: "ephemeral" }));

  expect(JSON.stringify(template.toJSON())).not.toContain("cdc-router-lambda");
  template.resourceCountIs("AWS::S3::Bucket", 0);
  template.resourceCountIs("AWS::Kinesis::Stream", 0);
  template.resourceCountIs("AWS::Lambda::EventSourceMapping", 2);
});

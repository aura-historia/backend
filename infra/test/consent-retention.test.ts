import * as cdk from "aws-cdk-lib";
import { Template } from "aws-cdk-lib/assertions";
import { readFileSync } from "node:fs";
import * as path from "node:path";
import { createApplicationStacks } from "../src/application-stack";
import { STAGES } from "../src/config";
import { workerQueueName } from "../src/worker-queue-config";

function daysFromServiceConstant(name: string): number {
  const source = readFileSync(path.join(__dirname, "../../src/user-service/src/ports/consent_workflow_cleanup.rs"), "utf8");
  const match = source.match(new RegExp(`pub const ${name}: i64 = (\\d+);`));
  expect(match).not.toBeNull();
  return Number(match![1]);
}

test.each(STAGES)("%s consent receipt retention covers the declared recovery paths", (stage) => {
  const app = new cdk.App({ analyticsReporting: false });
  const stacks = createApplicationStacks(app, { stage });
  const data = Template.fromStack(stacks.data);
  const compute = Template.fromStack(stacks.compute);
  const queue = (name: string) => {
    const matches = Object.values(data.findResources("AWS::SQS::Queue"))
      .filter((resource) => resource.Properties.QueueName === name);
    expect(matches).toHaveLength(1);
    return matches[0].Properties.MessageRetentionPeriod / 86400 as number;
  };
  const sourceDays = queue(workerQueueName("marketing-consent-sync", stage));
  const dlqDays = queue(workerQueueName("marketing-consent-sync", stage, true));
  const streams = Object.values(data.findResources("AWS::Kinesis::Stream"));
  expect(streams).toHaveLength(1);
  const kinesisDays = streams[0].Properties.RetentionPeriodHours / 24 as number;
  const archives = Object.values(compute.findResources("AWS::S3::Bucket"))
    .filter((resource) => resource.Properties.BucketName === `aura-historia-cdc-router-failures-${stage}`);
  expect(archives).toHaveLength(1);
  const archiveDays = archives[0].Properties.LifecycleConfiguration.Rules[0].ExpirationInDays as number;
  const receiptDays = daysFromServiceConstant("CONSENT_WORKFLOW_RECEIPT_RETENTION_DAYS");

  expect(receiptDays).toBeGreaterThan(archiveDays + sourceDays + dlqDays + kinesisDays);
  expect(receiptDays).toBeGreaterThan(daysFromServiceConstant("CONFIRMED_CHALLENGE_REPLAY_DAYS"));

  const dmsSelection = JSON.stringify(data.findResources("AWS::DMS::ReplicationTask"));
  expect(dmsSelection).not.toContain("newsletter_subscription_confirmations");
  expect(dmsSelection).not.toContain("loops_webhook_receipts");
});

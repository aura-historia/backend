import * as cdk from "aws-cdk-lib";
import { Match, Template } from "aws-cdk-lib/assertions";
import { createApplicationStacks } from "../src/application-stack";
import { STAGES } from "../src/config";

describe.each(STAGES)("%s account identity deletion", stage => {
  test("routes compact committed requests to an SQS worker without periodic scheduling", () => {
    const stacks = createApplicationStacks(new cdk.App({ analyticsReporting: false }), { stage });
    const compute = Template.fromStack(stacks.compute);
    compute.hasResourceProperties("AWS::Lambda::Function", {
      FunctionName: `user-identity-deletion-lambda-${stage}`,
      Timeout: 45,
      Environment: { Variables: { POSTGRES_MAX_CONNECTIONS: "1", AURA_HISTORIA_COGNITO_USER_POOL_ID: Match.anyValue() } },
    });
    const mapping = Object.values(compute.findResources("AWS::Lambda::EventSourceMapping"))
      .find(resource => JSON.stringify(resource.Properties.FunctionName).includes("UserIdentityDeletionVersion"));
    expect(mapping?.Properties).toMatchObject({ Enabled: true, BatchSize: 1, FunctionResponseTypes: ["ReportBatchItemFailures"] });
    expect(JSON.stringify(mapping?.Properties.EventSourceArn)).toContain("user-identity-deletion");
    expect(mapping?.DependsOn).toBeDefined();
    const schedules = Object.values(compute.findResources("AWS::Scheduler::Schedule"));
    expect(schedules).toHaveLength(4);
    expect(JSON.stringify(schedules)).not.toContain("user-identity-deletion");
    const cleanup = Object.values(compute.findResources("AWS::Lambda::Function"))
      .find(resource => resource.Properties.FunctionName === `backend-cleanup-lambda-${stage}`);
    expect(cleanup?.Properties.Environment.Variables.AURA_HISTORIA_COGNITO_USER_POOL_ID).toBeUndefined();

    const data = Template.fromStack(stacks.data);
    const task = Object.values(data.findResources("AWS::DMS::ReplicationTask"))[0];
    const tables = JSON.parse(task.Properties.TableMappings).rules
      .filter((rule: Record<string, string>) => rule["rule-type"] === "selection")
      .map((rule: Record<string, Record<string, string>>) => rule["object-locator"]["table-name"]);
    expect(tables).toContain("user_identity_deletion_requests");
    expect(tables).not.toContain("user_identity_deletions");
    expect(tables).not.toContain("user_cognito_identities");
  });
});

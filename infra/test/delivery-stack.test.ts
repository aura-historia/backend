import * as cdk from "aws-cdk-lib";
import { Template } from "aws-cdk-lib/assertions";
import { DeliveryStack } from "../src/delivery-stack";
import { createApplicationStacks } from "../src/application-stack";

function foundation(enabledTargets: readonly ("dev" | "prod")[] = ["prod"]) {
  return Template.fromStack(new DeliveryStack(new cdk.App({ analyticsReporting: false }), "delivery", {
    env: { account: "123456789012", region: "eu-central-1" }, enabledTargets,
    connectionArn: "arn:aws:codeconnections:eu-central-1:123456789012:connection/connection-id",
    controlKey: `control/${"a".repeat(40)}.zip`, controlVersion: "approved-control-version",
    workflowRevision: "a".repeat(40), buildConfigDigest: "b".repeat(64), buildConfig: "ci/delivery/build-config.json",
  }));
}
const template = foundation();

test("the reviewer reads only its target Anthropic key without model-service permissions", () => {
  const statements = Object.values(template.findResources("AWS::IAM::Policy"))
    .filter((policy) => JSON.stringify(policy.Properties.Roles).includes("prodGitHubPlanner"))
    .flatMap((policy) => policy.Properties.PolicyDocument.Statement)
    .filter((statement) => statement.Action === "ssm:GetParameter");
  expect(statements).toHaveLength(2);
  expect(JSON.stringify(statements[1].Resource)).toContain(":parameter/anthropic/prod/api-key");
  expect(JSON.stringify(template.toJSON())).not.toContain("bedrock:");
  expect(template.toJSON().Parameters).not.toHaveProperty("prodReviewerModelArn");
  expect(template.toJSON().Parameters).not.toHaveProperty("prodReviewerFoundationModelArns");
});

test("maintenance accepts observed DMS task identifiers without allowing wildcard task access", () => {
  const allowed = new RegExp(template.toJSON().Parameters.prodMaintenanceDmsTaskArn.AllowedPattern);
  for (const resource of ["liz4s522bnf7zipgqaix32x73m", "2PVREMWNPGYJCVU2IBPTOYTIV4", "approved-task"]) {
    expect(allowed.test(`arn:aws:dms:eu-central-1:550663032924:task:${resource}`)).toBe(true);
  }
  expect(allowed.test("arn:aws:dms:eu-central-1:550663032924:task:*")).toBe(false);
  expect(allowed.test("arn:aws:dms:eu-central-1:550663032924:rep:liz4s522bnf7zipgqaix32x73m")).toBe(false);
});

test("manual delivery has no source triggers, reserved builders, mandatory dev target or build VPC", () => {
  const pipelines = Object.values(template.findResources("AWS::CodePipeline::Pipeline"));
  expect(pipelines.map((resource) => resource.Properties.Name).sort()).toEqual(["aura-deploy-prod", "aura-release-build"]);
  for (const pipeline of pipelines) {
    expect(pipeline.Properties.ExecutionMode).toBe("QUEUED");
    expect(pipeline.Properties.PipelineType).toBe("V2");
    for (const action of pipeline.Properties.Stages[0].Actions) {
      if (action.ActionTypeId.Provider === "S3") expect(action.Configuration.PollForSourceChanges).toBe(false);
      else expect(action.Configuration.DetectChanges).toBe(false);
    }
  }
  for (const build of Object.values(template.findResources("AWS::CodeBuild::Project"))) {
    expect(build.Properties.VpcConfig).toBeUndefined();
    expect(build.Properties.Environment.Fleet).toBeUndefined();
    expect(build.Properties.ServiceRole["Fn::GetAtt"][1]).toBe("Arn");
    const buildspec = JSON.parse(build.Properties.Source.BuildSpec);
    if (build.Properties.Name.endsWith("-admission")) expect(buildspec.artifacts.files).toEqual(["prepared/**/*"]);
    else expect(buildspec.artifacts).toBeUndefined();
  }
  template.resourceCountIs("AWS::Events::Rule", 0);
  template.resourceCountIs("AWS::StepFunctions::StateMachine", 0);
  expect(Object.values(foundation(["dev", "prod"]).findResources("AWS::CodePipeline::Pipeline"))).toHaveLength(3);
});

test("native changesets, candidate migrations and final verification share one ordered exclusive stage", () => {
  const deploy = Object.values(template.findResources("AWS::CodePipeline::Pipeline"))
    .find((resource) => resource.Properties.Name === "aura-deploy-prod")!.Properties.Stages;
  expect(deploy.map((stage: any) => stage.Name)).toEqual(["Source", "Deploy"]);
  expect(deploy[1].Actions.map((action: any) => [action.Name, action.RunOrder])).toEqual([
    ["AuthorizedRequest", 1], ["RevalidateAndHold", 2], ["DeployMigrate", 3], ["RunCandidateMigrations", 4],
    ["DeployCompute", 5], ["DeployApi", 6], ["DeployObservability", 7], ["VerifyAndRecord", 8],
  ]);
  const native = deploy[1].Actions.filter((action: any) => action.ActionTypeId.Provider === "CloudFormation");
  expect(native.every((action: any) => action.Configuration.ActionMode.startsWith("#{Admission."))).toBe(true);
  expect(JSON.stringify(native[0].Configuration.RoleArn)).toContain("MigrationCloudFormationExecution");
  expect(JSON.stringify(native[1].Configuration.RoleArn)).not.toContain("MigrationCloudFormationExecution");
  expect(JSON.stringify(native)).not.toMatch(/application-prod-(network|data|identity|initialize)/);
});

test("requester trust and actual synthesized role grants cannot escalate into platform or migration administration", () => {
  const roles = template.findResources("AWS::IAM::Role");
  const policies = [...Object.values(template.findResources("AWS::IAM::Policy")),
    ...Object.values(template.findResources("AWS::IAM::ManagedPolicy"))];
  const statements = (roleId: string) => {
    const attached = new Set((roles[roleId].Properties.ManagedPolicyArns ?? []).map((reference: any) => reference.Ref));
    const managed = Object.entries(template.findResources("AWS::IAM::ManagedPolicy"))
      .filter(([id]) => attached.has(id)).map(([, policy]) => policy);
    return [...policies.filter((policy) => (JSON.stringify(policy.Properties.Roles) ?? "").includes(`"Ref":"${roleId}"`)), ...managed]
      .flatMap((policy) => policy.Properties.PolicyDocument.Statement);
  };
  for (const [id, role] of Object.entries(roles)) {
    if (id.includes("Requester")) {
      const trust = JSON.stringify(role.Properties.AssumeRolePolicyDocument);
      expect(trust).toContain("repo:aura-historia/backend:environment:aws-");
      const policy = JSON.stringify(statements(id));
      expect(policy).not.toMatch(/cloudformation:|iam:PassRole|sts:AssumeRole|codebuild:StartBuild|lambda:InvokeFunction/);
      expect(policy).not.toMatch(/dms:|rds:|cognito-idp:/);
    }
    if (id.includes("CloudFormationAction") && !id.includes("Default")) {
      const actual = statements(id);
      expect(actual).toEqual(expect.arrayContaining([expect.objectContaining({ Effect: "Deny",
        Action: expect.arrayContaining(["cloudformation:CreateStack", "cloudformation:DeleteStack"]) })]));
      expect(actual.some((entry: any) => entry.Effect === "Deny" && entry.Action === "iam:PassRole" && entry.NotResource)).toBe(true);
    }
    if (id.includes("CloudFormationExecution") && !id.includes("Migration")) {
      const actual = JSON.stringify(statements(id).filter((statement: any) => statement.Effect === "Allow"));
      expect(actual).not.toContain("role/aura-historia/migration/");
      expect(actual).not.toContain(":function:database-migration-lambda-");
      expect(actual).not.toContain(":function:database-inspection-lambda-");
      expect(actual).not.toContain(":function:database-bootstrap-lambda-");
      expect(actual).toContain("iam:PermissionsBoundary");
    }
  }
  const buckets = Object.values(template.findResources("AWS::S3::Bucket"));
  expect(buckets.every((bucket) => bucket.Properties.PublicAccessBlockConfiguration.BlockPublicAcls === true)).toBe(true);
  expect(buckets.some((bucket) => bucket.Properties.VersioningConfiguration?.Status === "Enabled")).toBe(true);
  expect(JSON.stringify(template.findResources("AWS::S3::BucketPolicy"))).toContain("s3:if-none-match");
});

test("protected ownership and alias rollout remain acyclic and use CloudFormation as sole owner", () => {
  const app = new cdk.App({ analyticsReporting: false });
  const stacks = createApplicationStacks(app, { stage: "prod" });
  const compute = Template.fromStack(stacks.compute);
  compute.resourceCountIs("AWS::Cognito::UserPool", 0);
  compute.resourceCountIs("AWS::SQS::Queue", 0);
  compute.resourceCountIs("AWS::S3::Bucket", 0);
  const identity = Template.fromStack(stacks.identity);
  identity.resourceCountIs("AWS::Cognito::UserPool", 1);
  identity.resourceCountIs("AWS::Lambda::Permission", 2);
  for (const permission of Object.values(identity.findResources("AWS::Lambda::Permission"))) {
    expect(permission.Condition).toBe("IdentityTriggersAttached");
    expect(permission.Properties.Principal).toBe("cognito-idp.amazonaws.com");
  }
  Template.fromStack(stacks.migration).resourceCountIs("AWS::Lambda::Function", 1);
  compute.resourceCountIs("AWS::CodeDeploy::DeploymentGroup", 2);
  const aliases = Object.values(compute.findResources("AWS::Lambda::Alias"));
  expect(aliases.filter((alias) => alias.UpdatePolicy?.CodeDeployLambdaAliasUpdate)).toHaveLength(2);
  for (const role of Object.values(compute.findResources("AWS::IAM::Role"))) {
    expect(JSON.stringify(role.Properties.PermissionsBoundary)).toContain("runtime-prod");
    expect(role.Properties.Path).toBe("/aura-historia/runtime/prod/");
  }
  expect(stacks.identity.dependencies).not.toContain(stacks.compute);
});


test("an application-only change leaves every protected template identical", () => {
  const before = createApplicationStacks(new cdk.App({ analyticsReporting: false }), { stage: "prod" });
  const after = createApplicationStacks(new cdk.App({ analyticsReporting: false }), { stage: "prod" });
  after.compute.lambdas.functions.auraHistoriaApi.addEnvironment("APPLICATION_ONLY_CONFIGURATION", "new-value");
  expect(Template.fromStack(after.compute).toJSON()).not.toEqual(Template.fromStack(before.compute).toJSON());
  for (const boundary of ["network", "data", "identity", "initialization"] as const)
    expect(Template.fromStack(after[boundary]).toJSON()).toEqual(Template.fromStack(before[boundary]).toJSON());
});

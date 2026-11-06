import * as cdk from "aws-cdk-lib";
import { Template } from "aws-cdk-lib/assertions";
import { createApplicationStacks } from "../src/application-stack";
import { STAGES, type StageName } from "../src/config";
import { CREDENTIAL_VERSIONS_METADATA, credentialSecrets } from "../src/credentials";
import { DeliveryStack } from "../src/delivery-stack";

function applicationTemplates(stage: StageName): Record<string, Record<string, any>> {
  const app = new cdk.App({ analyticsReporting: false });
  const stacks = createApplicationStacks(app, {
    stage,
    env: { account: "123456789012", region: "eu-central-1" },
  });
  return Object.fromEntries(Object.values(stacks)
    .filter((stack): stack is cdk.Stack => stack instanceof cdk.Stack)
    .map((stack) => [stack.stackName, Template.fromStack(stack).toJSON()]));
}

function dynamicReferences(template: unknown): string[] {
  const text = JSON.stringify(template);
  return [...text.matchAll(/\{\{resolve:(ssm|secretsmanager):([^:"}]+)/g)].map((match) => `${match[1]}:${match[2]}`);
}

test("operators provision exactly these runtime credential secrets and JSON fields", () => {
  expect(Object.values(credentialSecrets("prod")).map(({ secretName, fields }) => [secretName, fields])).toEqual([
    ["/aura-historia/prod/postgres/runtime", ["username", "password"]],
    ["/aura-historia/prod/app/stripe", ["apiKey"]],
    ["/aura-historia/prod/app/loops", ["apiKey", "webhookSigningSecret"]],
    ["/aura-historia/prod/app/opensearch", ["readerPassword", "productProjectorPassword"]],
    ["/aura-historia/prod/app/anthropic", ["apiKey"]],
    ["/aura-historia/prod/app/google", ["applicationCredentials"]],
    ["/aura-historia/prod/app/cloudflare", ["workersAiApiToken"]],
    ["/aura-historia/prod/app/fxratesapi", ["apiToken"]],
    ["/aura-historia/prod/app/cognito-identity-providers", ["googleClientSecret", "facebookClientSecret"]],
  ]);
});

describe.each(STAGES)("%s deploy-time credentials", (stage) => {
  const templates = applicationTemplates(stage);

  test("no plain SSM reference can resolve a credential into a template or change set", () => {
    const ssmPaths = Object.values(templates).flatMap(dynamicReferences)
      .filter((reference) => reference.startsWith("ssm:"));
    expect(ssmPaths.length).toBeGreaterThan(0);
    expect(ssmPaths.filter((reference) => /password|secret|api-key|api-token|token|credentials/i.test(reference)))
      .toEqual([]);
  });

  test("every Secrets Manager reference is pinned to a declared version parameter the planner can bind", () => {
    const pinned = new Set<string>();
    for (const [stackName, template] of Object.entries(templates)) {
      const declared: Record<string, string> = template.Metadata?.[CREDENTIAL_VERSIONS_METADATA] ?? {};
      for (const [parameter, secretName] of Object.entries(declared)) {
        expect(template.Parameters[parameter]).toMatchObject({ Type: "String", AllowedPattern: "^[A-Za-z0-9-]{32,64}$" });
        expect(secretName).toMatch(new RegExp(`^/aura-historia/${stage}/(postgres/runtime|app/[a-z0-9-]+)$`));
        pinned.add(`${stackName}:${secretName}`);
      }
      const references = JSON.stringify(template)
        .matchAll(/"\{\{resolve:secretsmanager:([^:"]+):SecretString:[A-Za-z]+::",\{"Ref":"([A-Za-z]+)"\},"\}\}"/g);
      const referenced = [...references].map(([, secretName, parameter]) => ({ secretName, parameter }));
      expect(referenced.length).toBe(dynamicReferences(template)
        .filter((reference) => reference.startsWith("secretsmanager:")).length);
      for (const { secretName, parameter } of referenced) {
        expect(declared[parameter]).toBe(secretName);
      }
    }
    const routine = (name: string) => /-(migrate|compute|api|observability)$/.test(name);
    expect([...pinned].filter((entry) => routine(entry.split(":")[0])).map((entry) => entry.split(":")[1]).sort())
      .toEqual(Object.values(credentialSecrets(stage))
        .map(({ secretName }) => secretName)
        .filter((name) => !name.endsWith("/fxratesapi") && !name.endsWith("/cognito-identity-providers"))
        .sort());
  });
});

describe("delivery permissions for deploy-time credentials", () => {
  const template = Template.fromStack(new DeliveryStack(new cdk.App({ analyticsReporting: false }), "delivery", {
    env: { account: "123456789012", region: "eu-central-1" }, enabledTargets: ["prod"],
    connectionArn: "arn:aws:codeconnections:eu-central-1:123456789012:connection/connection-id",
    controlKey: `control/${"a".repeat(40)}.zip`, controlVersion: "approved-control-version",
    workflowRevision: "a".repeat(40), buildConfigDigest: "b".repeat(64), buildConfig: "ci/delivery/build-config.json",
  }));
  const statementsFor = (role: string) => Object.values(template.findResources("AWS::IAM::Policy"))
    .filter((policy) => JSON.stringify(policy.Properties.Roles).includes(role))
    .flatMap((policy) => policy.Properties.PolicyDocument.Statement as Array<Record<string, any>>);
  const runtimeSecrets = [
    "secret:/aura-historia/prod/postgres/runtime-??????",
    "secret:/aura-historia/prod/app/*",
  ];

  test("CloudFormation may resolve only runtime and app secrets", () => {
    const secretStatements = statementsFor("prodCloudFormationExecution")
      .filter((statement) => JSON.stringify(statement.Action).includes("secretsmanager:"));
    expect(secretStatements.map((statement) => [statement.Effect, statement.Action])).toEqual([
      ["Deny", "secretsmanager:GetSecretValue"],
      ["Allow", "secretsmanager:GetSecretValue"],
    ]);
    for (const [statement, key] of [[secretStatements[0], "NotResource"], [secretStatements[1], "Resource"]] as const) {
      const resources = JSON.stringify(statement[key]);
      for (const secret of runtimeSecrets) expect(resources).toContain(secret);
      expect(resources).not.toMatch(/postgres\/(admin|migrator|inspector|replication)/);
    }
  });

  test("the planner reads secret metadata only and runtime roles cannot read secrets", () => {
    const plannerSecretStatements = statementsFor("prodGitHubPlanner")
      .filter((statement) => JSON.stringify(statement.Action).includes("secretsmanager:"));
    expect(plannerSecretStatements).toHaveLength(1);
    expect(plannerSecretStatements[0].Action).toBe("secretsmanager:DescribeSecret");
    for (const secret of runtimeSecrets) expect(JSON.stringify(plannerSecretStatements[0].Resource)).toContain(secret);
    const boundaries = Object.values(template.findResources("AWS::IAM::ManagedPolicy"));
    const boundary = (name: string) => JSON.stringify(boundaries
      .find((policy) => policy.Properties.ManagedPolicyName === name)!.Properties.PolicyDocument);
    expect(boundary("runtime-prod")).not.toContain("secretsmanager:");
    expect(boundary("runtime-prod")).not.toContain("ssm:GetParameters");
    expect(boundary("migration-prod")).toContain("secret:/aura-historia/prod/postgres/migrator-??????");
  });
});

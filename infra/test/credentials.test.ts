import * as cdk from "aws-cdk-lib";
import { Template } from "aws-cdk-lib/assertions";
import { createApplicationStacks } from "../src/application-stack";
import { STAGES, type StageName } from "../src/config";
import { CREDENTIAL_VERSIONS_METADATA, credentialSecrets } from "../src/credentials";

function applicationTemplates(stage: StageName): Record<string, Record<string, any>> {
  const app = new cdk.App({ analyticsReporting: false });
  const stacks = createApplicationStacks(app, {
    stage,
    env: { account: "123456789012", region: "eu-central-1" },
  });
  return Object.fromEntries(Object.values(stacks).map((stack) => [stack.stackName, Template.fromStack(stack).toJSON()]));
}

function dynamicReferences(template: unknown): string[] {
  const text = JSON.stringify(template);
  return [...text.matchAll(/\{\{resolve:(ssm|secretsmanager):([^:"}]+)/g)].map((match) => `${match[1]}:${match[2]}`);
}

test("operators provision or the host stack generates exactly these deploy-time secrets and JSON fields", () => {
  expect(Object.values(credentialSecrets("prod")).map(({ secretName, fields }) => [secretName, fields])).toEqual([
    ["/aura-historia/prod/edge/origin-verify", ["value"]],
    ["/aura-historia/prod/edge/post-confirmation-signing", ["value"]],
    ["/aura-historia/prod/app/cognito-identity-providers", ["googleClientSecret", "facebookClientSecret"]],
  ]);
});

describe.each(STAGES)("%s deploy-time credentials", (stage) => {
  const templates = applicationTemplates(stage);

  test("no plain SSM reference can resolve a credential into a template or change set", () => {
    const ssmPaths = Object.values(templates).flatMap(dynamicReferences)
      .filter((reference) => reference.startsWith("ssm:"));
    expect(ssmPaths.length).toBeGreaterThan(0);
    expect(ssmPaths.filter((reference) => /password|secret|api-key|api-token|token|credentials|verify/i.test(reference)))
      .toEqual([]);
  });

  test("every Secrets Manager reference is pinned to a declared version parameter", () => {
    const pinned: string[] = [];
    for (const [stackName, template] of Object.entries(templates)) {
      const declared: Record<string, string> = template.Metadata?.[CREDENTIAL_VERSIONS_METADATA] ?? {};
      for (const [parameter, secretName] of Object.entries(declared)) {
        expect(template.Parameters[parameter]).toMatchObject({ Type: "String", AllowedPattern: "^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$" });
        pinned.push(`${stackName}:${secretName}`);
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
    expect(pinned.sort()).toEqual([
      `application-${stage}-api:/aura-historia/${stage}/edge/origin-verify`,
      `application-${stage}-identity:/aura-historia/${stage}/app/cognito-identity-providers`,
      `application-${stage}-identity:/aura-historia/${stage}/edge/post-confirmation-signing`,
    ]);
  });
});

import * as cdk from "aws-cdk-lib";
import { Template } from "aws-cdk-lib/assertions";
import { createApplicationStacks } from "../src/application-stack";
import { STAGES, type StageName } from "../src/config";

type CloudFormationResource = {
  readonly Properties: Record<string, unknown>;
};

function computeTemplate(stage: StageName): Template {
  const app = new cdk.App({ analyticsReporting: false });
  return Template.fromStack(createApplicationStacks(app, { stage }).compute);
}

function apiFunction(template: Template, stage: StageName): CloudFormationResource {
  const functions = Object.values(template.findResources("AWS::Lambda::Function")) as CloudFormationResource[];
  const functionResource = functions.find((resource) => resource.Properties.FunctionName === `aura-historia-api-${stage}`);
  if (!functionResource) {
    throw new Error(`Missing aura-historia-api-${stage} Lambda.`);
  }
  return functionResource;
}

describe.each(STAGES)("%s API Lambda", (stage) => {
  test("packages one ordinary Axum HTTP function with deliberate request limits", () => {
    const template = computeTemplate(stage);
    const functionResource = apiFunction(template, stage);
    const environment = functionResource.Properties.Environment as { Variables: Record<string, unknown> };

    expect(functionResource.Properties).toMatchObject({
      Architectures: ["x86_64"],
      Handler: "lib.handler",
      MemorySize: 512,
      Runtime: "provided.al2023",
      Timeout: 15,
    });
    expect(JSON.stringify(functionResource.Properties.Code)).toContain(`aura-historia-api-${stage}-`);
    expect(environment.Variables.STAGE).toBe(stage);
    expect(environment.Variables.AWS_LAMBDA_HTTP_IGNORE_STAGE_IN_PATH).toBe("true");
    expect(environment.Variables.AURA_HISTORIA_COGNITO_ISSUER).toBeDefined();
    expect(environment.Variables.AURA_HISTORIA_COGNITO_JWKS_URL).toBeDefined();
    expect(environment.Variables.AURA_HISTORIA_COGNITO_APP_CLIENT_IDS).toBeDefined();
    expect(environment.Variables.AURA_HISTORIA_COGNITO_USER_POOL_ID).toBeDefined();
    expect(environment.Variables.OPENSEARCH_ENDPOINT_URL).toBeDefined();
    expect(environment.Variables.POSTGRES_MAX_CONNECTIONS).toBe("1");
    expect(functionResource.Properties.ReservedConcurrentExecutions).toBeUndefined();
  });

  test("keeps the function private to its execution boundary", () => {
    const template = computeTemplate(stage);
    const functionResource = apiFunction(template, stage);

    const functions = template.findResources("AWS::Lambda::Function");
    const apiFunctionLogicalId = Object.entries(functions).find(
      ([, resource]) => (resource as CloudFormationResource).Properties.FunctionName === `aura-historia-api-${stage}`,
    )?.[0];
    const eventSourceMappings = Object.values(template.findResources("AWS::Lambda::EventSourceMapping"));
    const aliases = Object.values(template.findResources("AWS::Lambda::Alias"));

    expect(apiFunctionLogicalId).toBeDefined();
    expect(functionResource.Properties.VpcConfig === undefined).toBe(stage === "ephemeral");
    expect(JSON.stringify(eventSourceMappings)).not.toContain(apiFunctionLogicalId);
    expect(JSON.stringify(aliases)).not.toContain(apiFunctionLogicalId);
    expect(Object.values(template.findResources("AWS::Lambda::Url"))).toHaveLength(0);
    expect(Object.values(template.findResources("AWS::ApiGatewayV2::Integration"))).toHaveLength(0);
  });
});

test("grants only API session-revocation actions against the Cognito user pool", () => {
  const template = computeTemplate("dev");
  const policies = Object.values(template.findResources("AWS::IAM::Policy")) as CloudFormationResource[];
  const apiPolicy = policies.find((policy) => {
    const serialized = JSON.stringify(policy);
    return serialized.includes("cognito-idp:AdminUserGlobalSignOut") && serialized.includes("cognito-idp:ListUsers");
  });

  expect(apiPolicy).toBeDefined();
  const serialized = JSON.stringify(apiPolicy);
  expect(serialized).not.toContain("cognito-idp:*");
  expect(serialized).not.toContain("secretsmanager:GetSecretValue");
});

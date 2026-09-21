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

function lambdaFunction(template: Template, functionName: string): CloudFormationResource {
  const functions = Object.values(template.findResources("AWS::Lambda::Function")) as CloudFormationResource[];
  const functionResource = functions.find((resource) => resource.Properties.FunctionName === functionName);
  if (!functionResource) {
    throw new Error(`Missing ${functionName} Lambda.`);
  }
  return functionResource;
}

function apiFunction(template: Template, stage: StageName): CloudFormationResource {
  return lambdaFunction(template, `aura-historia-api-${stage}`);
}

function expectedApiEnvironmentKeys(stage: StageName): string[] {
  const keys = [
    "AURA_HISTORIA_COGNITO_APP_CLIENT_IDS",
    "AURA_HISTORIA_COGNITO_ISSUER",
    "AURA_HISTORIA_COGNITO_JWKS_URL",
    "AURA_HISTORIA_COGNITO_USER_POOL_ID",
    "AURA_HISTORIA_GOOGLE_ADC_CREDENTIALS_JSON",
    "AWS_LAMBDA_HTTP_IGNORE_STAGE_IN_PATH",
    "OPENSEARCH_ENDPOINT_URL",
    "POSTGRES_DATABASE",
    "POSTGRES_HOST",
    "POSTGRES_MAX_CONNECTIONS",
    "POSTGRES_PORT",
    "POSTGRES_TLS_ROOT_CERT",
    "STAGE",
    "STRIPE_API_KEY",
    "STRIPE_CHECKOUT_CANCEL_URL",
    "STRIPE_CHECKOUT_SUCCESS_URL",
    "STRIPE_PORTAL_RETURN_URL",
    "STRIPE_PRO_MONTHLY_PRICE_ID",
    "STRIPE_PRO_YEARLY_PRICE_ID",
    "STRIPE_ULTIMATE_MONTHLY_PRICE_ID",
    "STRIPE_ULTIMATE_YEARLY_PRICE_ID",
    "VERTEX_AI_LOCATION",
    "VERTEX_AI_PROJECT_ID",
    "ZOHO_ACCOUNTS_URL",
    "ZOHO_CAMPAIGNS_URL",
    "ZOHO_CLIENT_ID",
    "ZOHO_CLIENT_SECRET",
    "ZOHO_LIST_KEY",
    "ZOHO_REFRESH_TOKEN",
  ];

  if (stage === "ephemeral") {
    keys.push("POSTGRES_PASSWORD", "POSTGRES_USERNAME");
  } else {
    keys.push("OPENSEARCH_PASSWORD", "OPENSEARCH_USERNAME", "POSTGRES_SECRET_ARN");
  }
  return keys.sort();
}

function expectedVertexEnvironment(stage: StageName): Record<string, string> {
  return stage === "ephemeral"
    ? {
        AURA_HISTORIA_GOOGLE_ADC_CREDENTIALS_JSON: "{\"type\":\"service_account\",\"project_id\":\"aura-historia-ephemeral-test\"}",
        VERTEX_AI_LOCATION: "eu",
        VERTEX_AI_PROJECT_ID: "aura-historia-ephemeral-test",
      }
    : {
        AURA_HISTORIA_GOOGLE_ADC_CREDENTIALS_JSON: `{{resolve:ssm:/secrets/${stage}/google-application-credentials}}`,
        VERTEX_AI_LOCATION: `{{resolve:ssm:/vertex-ai/${stage}/location}}`,
        VERTEX_AI_PROJECT_ID: `{{resolve:ssm:/vertex-ai/${stage}/project-id}}`,
      };
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
    expect(environment.Variables.POSTGRES_SECRET_ARN === undefined).toBe(stage === "ephemeral");
    expect(environment.Variables.POSTGRES_USERNAME === undefined).toBe(stage !== "ephemeral");
    expect(environment.Variables.POSTGRES_PASSWORD === undefined).toBe(stage !== "ephemeral");
    expect(Object.keys(environment.Variables).sort()).toEqual(expectedApiEnvironmentKeys(stage));
    expect(environment.Variables).toMatchObject(expectedVertexEnvironment(stage));
    expect(environment.Variables.GOOGLE_APPLICATION_CREDENTIALS).toBeUndefined();
    expect(functionResource.Properties.ReservedConcurrentExecutions).toBeUndefined();
  });

  test("keeps Vertex ADC configuration and permissions out of the projector", () => {
    const template = computeTemplate(stage);
    const projector = lambdaFunction(template, `product-listing-opensearch-lambda-${stage}`);
    const projectorEnvironment = projector.Properties.Environment as { Variables: Record<string, unknown> };

    expect(projectorEnvironment.Variables.AURA_HISTORIA_GOOGLE_ADC_CREDENTIALS_JSON).toBeUndefined();
    expect(projectorEnvironment.Variables.GOOGLE_APPLICATION_CREDENTIALS).toBeUndefined();
    expect(projectorEnvironment.Variables.VERTEX_AI_LOCATION).toBeUndefined();
    expect(projectorEnvironment.Variables.VERTEX_AI_PROJECT_ID).toBeUndefined();
    expect(JSON.stringify(projector.Properties)).not.toContain("google-application-credentials");
    expect(JSON.stringify(projector.Properties)).not.toContain("vertex-ai");
    expect(JSON.stringify(template.findResources("AWS::IAM::Policy"))).not.toContain("ssm:GetParameter");
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
});

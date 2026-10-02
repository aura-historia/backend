import * as cdk from "aws-cdk-lib";
import { Template } from "aws-cdk-lib/assertions";
import { ApplicationEphemeralStack, createApplicationStacks } from "../src/application-stack";
import { stageConfig, type StageName } from "../src/config";

type Resource = {
  readonly DependsOn?: string[] | string;
  readonly Properties: Record<string, any>;
};

function computeTemplate(stage: StageName): Template {
  const app = new cdk.App({ analyticsReporting: false });
  return Template.fromStack(createApplicationStacks(app, { stage }).compute);
}

function resourceByFunctionName(template: Template, functionName: string): Resource {
  const functions = Object.values(template.findResources("AWS::Lambda::Function")) as Resource[];
  const resource = functions.find((functionResource) => functionResource.Properties.FunctionName === functionName);
  if (!resource) throw new Error(`Missing Lambda function ${functionName}.`);
  return resource;
}

function resourceByType(template: Template, type: string): [string, Resource][] {
  return Object.entries(template.findResources(type)) as [string, Resource][];
}

describe.each(["dev", "prod"] as const)("%s Cognito federation", (stage) => {
  test("keeps the native pool contract and configures both Cognito triggers", () => {
    const template = computeTemplate(stage);
    template.resourceCountIs("AWS::Cognito::UserPool", 1);

    const [pool] = resourceByType(template, "AWS::Cognito::UserPool").map(([, resource]) => resource);
    expect(pool.Properties.Schema).toEqual(
      expect.arrayContaining([
        expect.objectContaining({ Name: "email", Required: true, Mutable: true }),
        expect.objectContaining({ Name: "given_name", Required: false, Mutable: true }),
        expect.objectContaining({ Name: "family_name", Required: false, Mutable: true }),
        expect.objectContaining({ Name: "locale", Required: false, Mutable: true }),
      ]),
    );
    expect(pool.Properties.AutoVerifiedAttributes).toEqual(["email"]);
    expect(pool.Properties.UsernameConfiguration).toEqual({ CaseSensitive: true });
    expect(pool.Properties.Policies.PasswordPolicy).toMatchObject({
      MinimumLength: 8,
      RequireLowercase: true,
      RequireNumbers: true,
      RequireSymbols: true,
      RequireUppercase: true,
      TemporaryPasswordValidityDays: 7,
    });
    expect(pool.Properties.AccountRecoverySetting.RecoveryMechanisms).toEqual([
      { Name: "verified_email", Priority: 1 },
    ]);
    expect(pool.Properties.LambdaConfig.PostConfirmation).toBeDefined();
    expect(pool.Properties.LambdaConfig.PreSignUp).toBeDefined();
  });

  test("creates Google with secure credentials and the narrow mapped profile", () => {
    const template = computeTemplate(stage);
    template.resourceCountIs("AWS::Cognito::UserPoolIdentityProvider", 1);
    const [[, provider]] = resourceByType(template, "AWS::Cognito::UserPoolIdentityProvider");

    expect(provider.Properties).toMatchObject({
      ProviderName: "Google",
      ProviderType: "Google",
      ProviderDetails: {
        client_id: `{{resolve:ssm:/cognito/${stage}/identity-providers/google/client-id}}`,
        client_secret: `{{resolve:ssm-secure:/cognito/${stage}/identity-providers/google/client-secret}}`,
        authorize_scopes: "openid email profile",
      },
      AttributeMapping: {
        email: "email",
        email_verified: "email_verified",
        given_name: "given_name",
        family_name: "family_name",
        locale: "locale",
      },
    });

  });

  test("keeps the public app client native, secretless, authorization-code-only, and ordered after providers", () => {
    const template = computeTemplate(stage);
    template.resourceCountIs("AWS::Cognito::UserPoolClient", 1);
    const [[, client]] = resourceByType(template, "AWS::Cognito::UserPoolClient");
    const [[providerId]] = resourceByType(template, "AWS::Cognito::UserPoolIdentityProvider");
    const config = stageConfig(stage);

    expect(client.Properties).toMatchObject({
      GenerateSecret: false,
      EnableTokenRevocation: true,
      SupportedIdentityProviders: ["COGNITO", "Google"],
      AllowedOAuthFlowsUserPoolClient: true,
      AllowedOAuthFlows: ["code"],
      AllowedOAuthScopes: ["openid", "email", "profile"],
      CallbackURLs: config.cognitoCallbackUrls,
      LogoutURLs: config.cognitoLogoutUrls,
      ExplicitAuthFlows: expect.arrayContaining(["ALLOW_USER_PASSWORD_AUTH", "ALLOW_USER_SRP_AUTH"]),
      AccessTokenValidity: 60,
      IdTokenValidity: 60,
      RefreshTokenValidity: 43200,
      TokenValidityUnits: { AccessToken: "minutes", IdToken: "minutes", RefreshToken: "minutes" },
      ReadAttributes: expect.arrayContaining(["email", "email_verified", "given_name", "family_name", "locale"]),
    });
    expect(client.DependsOn ?? []).toEqual(expect.arrayContaining([providerId]));
  });

  test("uses a non-VPC pre-sign-up Lambda with only the account-linking IAM actions", () => {
    const template = computeTemplate(stage);
    const preSignUp = resourceByFunctionName(template, `cognito-pre-sign-up-${stage}`);
    const variables = preSignUp.Properties.Environment.Variables as Record<string, string>;

    expect(preSignUp.Properties.VpcConfig).toBeUndefined();
    expect(Object.keys(variables).some((key) => key.startsWith("POSTGRES_") || key.includes("DATABASE"))).toBe(false);
    expect(variables.COGNITO_IDENTITY_PROVIDER_LINKING_POLICY).toContain('"providerName":"Google"');
    expect(JSON.stringify(variables)).not.toContain("client-secret");

    const policies = Object.values(template.findResources("AWS::IAM::Policy")) as Resource[];
    const linkingPolicy = policies.find((policy) => JSON.stringify(policy).includes("cognito-idp:AdminLinkProviderForUser"));
    expect(linkingPolicy).toBeDefined();
    const statements = linkingPolicy!.Properties.PolicyDocument.Statement as Array<Record<string, unknown>>;
    const linkingStatement = statements.find((statement) =>
      JSON.stringify(statement.Action).includes("cognito-idp:AdminLinkProviderForUser"),
    );
    expect(linkingStatement).toBeDefined();
    expect(linkingStatement!.Action).toEqual(["cognito-idp:ListUsers", "cognito-idp:AdminLinkProviderForUser"]);
    expect(JSON.stringify(linkingStatement!.Resource)).toContain(":cognito-idp:");
    expect(JSON.stringify(linkingStatement!.Resource)).toContain(":userpool/*");
    expect(JSON.stringify(linkingStatement)).not.toContain("cognito-idp:*");
  });
});

test("ephemeral Cognito follows its empty provider catalog", () => {
  const app = new cdk.App({ analyticsReporting: false });
  const stack = new ApplicationEphemeralStack(app, "cognito-ephemeral", { stage: "ephemeral" });
  const template = Template.fromStack(stack);
  template.resourceCountIs("AWS::Cognito::UserPool", 1);
  template.resourceCountIs("AWS::Cognito::UserPoolIdentityProvider", 0);

  expect(
    Object.values(template.findResources("AWS::Lambda::Function")).some(
      (resource) => resource.Properties.FunctionName === "cognito-pre-sign-up-ephemeral",
    ),
  ).toBe(false);
  const [[, pool]] = resourceByType(template, "AWS::Cognito::UserPool");
  expect(pool.Properties.LambdaConfig.PostConfirmation).toBeDefined();
  expect(pool.Properties.LambdaConfig.PreSignUp).toBeUndefined();
  const [[, client]] = resourceByType(template, "AWS::Cognito::UserPoolClient");
  expect(client.Properties.SupportedIdentityProviders).toEqual(["COGNITO"]);
  expect(stageConfig("ephemeral").cognitoIdentityProviders).toEqual([]);
});

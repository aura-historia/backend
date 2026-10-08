import * as cdk from "aws-cdk-lib";
import { Template } from "aws-cdk-lib/assertions";
import { createApplicationStacks } from "../src/application-stack";
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
  test("uses the generated static English verification email with Cognito's code placeholder", () => {
    const template = computeTemplate(stage);
    const [[, pool]] = resourceByType(template, "AWS::Cognito::UserPool");
    const verification = pool.Properties.VerificationMessageTemplate;

    expect(verification.EmailSubject).toBe("Verify your email");
    expect(verification.EmailMessage).toContain("GENERATED FILE: Source mjml/cognito/verification/en.mjml");
    expect(verification.EmailMessage).toContain(
      "Confirm your email address to complete your Aura Historia registration. If you selected the newsletter option during registration, this confirmation also verifies the email address for that subscription.",
    );
    expect(verification.EmailMessage).toContain("{####}");
    expect(verification.EmailMessage).toContain("complete your signup or reset your password");
    expect(verification.EmailMessage).not.toContain("Go to Login");
    expect(verification.EmailMessage).not.toContain("upgrade");
  });

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
        expect.objectContaining({ Name: "marketing_consent", AttributeDataType: "String", Mutable: false }),
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

  test("creates Google with an SSM String client secret and the narrow mapped profile", () => {
    const template = computeTemplate(stage);
    template.resourceCountIs("AWS::Cognito::UserPoolIdentityProvider", 2);
    const provider = resourceByType(template, "AWS::Cognito::UserPoolIdentityProvider")
      .map(([, resource]) => resource)
      .find((resource) => resource.Properties.ProviderName === "Google");
    expect(provider).toBeDefined();

    expect(provider!.Properties).toMatchObject({
      ProviderName: "Google",
      ProviderType: "Google",
      ProviderDetails: {
        client_id: `{{resolve:ssm:/cognito/${stage}/identity-providers/google/client-id}}`,
        client_secret: `{{resolve:ssm:/cognito/${stage}/identity-providers/google/client-secret}}`,
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
    expect(provider!.Properties.AttributeMapping).not.toHaveProperty("custom:marketing_consent");

  });

  test("creates Facebook with only the email and public profile mappings", () => {
    const template = computeTemplate(stage);
    const provider = resourceByType(template, "AWS::Cognito::UserPoolIdentityProvider")
      .map(([, resource]) => resource)
      .find((resource) => resource.Properties.ProviderName === "Facebook");

    expect(provider).toBeDefined();
    expect(provider!.Properties).toMatchObject({
      ProviderName: "Facebook",
      ProviderType: "Facebook",
      ProviderDetails: {
        client_id: `{{resolve:ssm:/cognito/${stage}/identity-providers/facebook/client-id}}`,
        client_secret: `{{resolve:ssm:/cognito/${stage}/identity-providers/facebook/client-secret}}`,
        api_version: "v26.0",
        authorize_scopes: "email,public_profile",
      },
      AttributeMapping: {
        email: "email",
        given_name: "first_name",
        family_name: "last_name",
      },
    });
    expect(provider!.Properties.AttributeMapping).not.toHaveProperty("email_verified");
    expect(provider!.Properties.AttributeMapping).not.toHaveProperty("custom:marketing_consent");
  });

  test("keeps the public app client native, secretless, authorization-code-only, and ordered after providers", () => {
    const template = computeTemplate(stage);
    template.resourceCountIs("AWS::Cognito::UserPoolClient", 1);
    const [[, client]] = resourceByType(template, "AWS::Cognito::UserPoolClient");
    const providerIds = resourceByType(template, "AWS::Cognito::UserPoolIdentityProvider").map(([id]) => id);
    const config = stageConfig(stage);

    expect(client.Properties).toMatchObject({
      GenerateSecret: false,
      EnableTokenRevocation: true,
      SupportedIdentityProviders: ["COGNITO", "Google", "Facebook"],
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
      WriteAttributes: expect.arrayContaining([
        "email",
        "given_name",
        "family_name",
        "locale",
        "custom:marketing_consent",
      ]),
    });
    expect(client.Properties.WriteAttributes).not.toContain("email_verified");
    expect(client.Properties.ReadAttributes).not.toContain("custom:marketing_consent");
    expect(client.DependsOn ?? []).toEqual(expect.arrayContaining(providerIds));
  });

  test("uses a PostgreSQL-backed five-second pre-sign-up Lambda with narrow federation access", () => {
    const template = computeTemplate(stage);
    const preSignUp = resourceByFunctionName(template, `cognito-pre-sign-up-${stage}`);
    const variables = preSignUp.Properties.Environment.Variables as Record<string, string>;

    expect(preSignUp.Properties.Timeout).toBe(5);
    expect(preSignUp.Properties.VpcConfig).toBeDefined();
    expect(variables.POSTGRES_MAX_CONNECTIONS).toBe("1");
    expect(variables.POSTGRES_SECRET_ARN).toBeDefined();
    expect(variables.POSTGRES_TLS_ROOT_CERT).toBeDefined();
    const providerPolicies = JSON.parse(variables.COGNITO_PROVIDER_SIGNUP_POLICY) as Array<Record<string, string>>;
    expect(providerPolicies).toEqual([
      {
        providerName: "Google",
        existingEmailAction: "LINK_VERIFIED",
        linkSourceAttributeName: "Cognito_Subject",
      },
      { providerName: "Facebook", existingEmailAction: "REJECT" },
    ]);
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

  test("keeps post-confirmation local to PostgreSQL without provider or delivery permissions", () => {
    const template = computeTemplate(stage);
    const postConfirmation = resourceByFunctionName(template, `cognito-post-confirmation-${stage}`);
    const environment = postConfirmation.Properties.Environment.Variables as Record<string, string>;

    expect(JSON.stringify(environment)).not.toMatch(/loops|ses|sqs|step.?functions|consent.?queue/i);
    const roleId = (postConfirmation.Properties.Role as { "Fn::GetAtt": [string, string] })["Fn::GetAtt"][0];
    const policies = Object.values(template.findResources("AWS::IAM::Policy")) as Resource[];
    const rolePolicies = policies.filter((policy) =>
      (policy.Properties.Roles as Array<{ Ref: string }> | undefined)?.some((role) => role.Ref === roleId),
    );

    expect(JSON.stringify(rolePolicies)).not.toMatch(/ses:|sqs:|states:/i);
  });
});

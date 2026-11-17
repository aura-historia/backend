import * as cdk from "aws-cdk-lib";
import { Template } from "aws-cdk-lib/assertions";
import { createApplicationStacks } from "../src/application-stack";
import { STAGES, stageConfig, type StageName } from "../src/config";
import { credentialSecrets } from "../src/credentials";
import { credentialReference } from "./credential-reference";

type Resource = {
  readonly DependsOn?: string[] | string;
  readonly Properties: Record<string, any>;
};

function identityTemplate(stage: StageName): Template {
  const app = new cdk.App({ analyticsReporting: false });
  return Template.fromStack(createApplicationStacks(app, { stage }).identity);
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

describe.each(STAGES)("%s Cognito federation", (stage) => {
  test("uses the generated static English verification email with Cognito's code placeholder", () => {
    const template = identityTemplate(stage);
    const [[, pool]] = resourceByType(template, "AWS::Cognito::UserPool");
    const verification = pool.Properties.VerificationMessageTemplate;

    expect(verification.EmailSubject).toBe("Verify your email");
    expect(verification.EmailMessage).toContain("GENERATED FILE: Source mjml/cognito/verification/en.mjml");
    expect(verification.EmailMessage).toContain(
      "Enter this code in Aura Historia to confirm your email address and finish your registration. If you chose the newsletter when signing up, this also confirms your subscription.",
    );
    expect(verification.EmailMessage).toContain("{####}");
    expect(verification.EmailMessage).toContain("The same code also completes a password reset.");
    // Cognito rejects verification messages longer than 20,000 characters.
    expect(verification.EmailMessage.length).toBeLessThanOrEqual(20_000);
    expect(verification.EmailMessage).not.toContain("Go to Login");
    expect(verification.EmailMessage).not.toContain("upgrade");
  });

  test("keeps the native pool contract and configures both Cognito triggers", () => {
    const template = identityTemplate(stage);
    template.resourceCountIs("AWS::Cognito::UserPool", 1);

    const [pool] = resourceByType(template, "AWS::Cognito::UserPool").map(([, resource]) => resource);
    expect(pool.Properties.Schema).toEqual([
      { Name: "email", Required: true, Mutable: true },
      { Name: "given_name", Required: false, Mutable: true },
      { Name: "family_name", Required: false, Mutable: true },
      { Name: "locale", Required: false, Mutable: true },
      { Name: "marketing_consent", AttributeDataType: "String", Mutable: false },
    ]);
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
    const aliases = Object.fromEntries(resourceByType(template, "AWS::Lambda::Alias")
      .map(([id, alias]) => [alias.Properties.FunctionName.Ref as string, id]));
    const functions = Object.fromEntries(resourceByType(template, "AWS::Lambda::Function")
      .map(([id, fn]) => [fn.Properties.FunctionName as string, id]));
    expect(pool.Properties.LambdaConfig).toEqual({
      PostConfirmation: { Ref: aliases[functions[`cognito-post-confirmation-forwarder-${stage}`]] },
      PreSignUp: { Ref: aliases[functions[`cognito-pre-sign-up-trigger-${stage}`]] },
    });
    // Cognito may invoke each trigger alias, and only on behalf of this pool.
    const [[poolId]] = resourceByType(template, "AWS::Cognito::UserPool");
    const permissions = resourceByType(template, "AWS::Lambda::Permission").map(([, permission]) => permission.Properties);
    expect(permissions).toHaveLength(2);
    for (const permission of permissions) {
      expect(permission).toMatchObject({
        Action: "lambda:InvokeFunction",
        Principal: "cognito-idp.amazonaws.com",
        SourceArn: { "Fn::GetAtt": [poolId, "Arn"] },
      });
    }
  });

  test("creates Google with a version-pinned client secret and the narrow mapped profile", () => {
    const template = identityTemplate(stage);
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
        client_secret: credentialReference(credentialSecrets(stage).cognitoIdentityProviders, "googleClientSecret"),
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

  test("pins Facebook v21.0 with only the email and public profile mappings", () => {
    const template = identityTemplate(stage);
    const provider = resourceByType(template, "AWS::Cognito::UserPoolIdentityProvider")
      .map(([, resource]) => resource)
      .find((resource) => resource.Properties.ProviderName === "Facebook");

    expect(provider).toBeDefined();
    expect(provider!.Properties).toMatchObject({
      ProviderName: "Facebook",
      ProviderType: "Facebook",
      ProviderDetails: {
        client_id: `{{resolve:ssm:/cognito/${stage}/identity-providers/facebook/client-id}}`,
        client_secret: credentialReference(credentialSecrets(stage).cognitoIdentityProviders, "facebookClientSecret"),
        api_version: "v21.0",
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
    const template = identityTemplate(stage);
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

  test("runs a five-second pre-sign-up trigger outside any VPC with narrow federation access", () => {
    const template = identityTemplate(stage);
    const preSignUp = resourceByFunctionName(template, `cognito-pre-sign-up-trigger-${stage}`);
    const variables = preSignUp.Properties.Environment.Variables as Record<string, unknown>;

    expect(preSignUp.Properties).toMatchObject({ Timeout: 5, MemorySize: 256, ReservedConcurrentExecutions: 5 });
    expect(preSignUp.Properties.VpcConfig).toBeUndefined();
    expect(preSignUp.Properties.Layers).toBeUndefined();
    expect(Object.keys(variables).sort()).toEqual(["COGNITO_PROVIDER_SIGNUP_POLICY", "LOG_LEVEL"]);
    const providerPolicies = JSON.parse(variables.COGNITO_PROVIDER_SIGNUP_POLICY as string) as Array<Record<string, string>>;
    expect(providerPolicies).toEqual([
      {
        providerName: "Google",
        existingEmailAction: "LINK_VERIFIED",
        linkSourceAttributeName: "Cognito_Subject",
      },
      { providerName: "Facebook", existingEmailAction: "REJECT" },
    ]);
    expect(JSON.stringify(variables)).not.toMatch(/ClientSecret|cognito-identity-providers|POSTGRES/);

    const statements = rolePolicyStatements(template, preSignUp);
    expect(statements).toHaveLength(1);
    expect(statements[0].Action).toEqual(["cognito-idp:ListUsers", "cognito-idp:AdminLinkProviderForUser"]);
    expect(JSON.stringify(statements[0].Resource)).toContain(":cognito-idp:");
    expect(JSON.stringify(statements[0].Resource)).toContain(":userpool/*");
  });

  test("runs a post-confirmation forwarder outside any VPC with no AWS permissions of its own", () => {
    const template = identityTemplate(stage);
    const forwarder = resourceByFunctionName(template, `cognito-post-confirmation-forwarder-${stage}`);
    expect(forwarder.Properties).toMatchObject({ Timeout: 5, MemorySize: 256, ReservedConcurrentExecutions: 5 });
    expect(forwarder.Properties.VpcConfig).toBeUndefined();
    expect(rolePolicyStatements(template, forwarder)).toEqual([]);
  });

  test("pins each trigger's code to a completed release object and its version to the code digest", () => {
    const template = identityTemplate(stage);
    for (const [functionName, stem] of [
      [`cognito-pre-sign-up-trigger-${stage}`, "CognitoPreSignUp"],
      [`cognito-post-confirmation-forwarder-${stage}`, "CognitoPostConfirmationForwarder"],
    ]) {
      const fn = resourceByFunctionName(template, functionName);
      expect(fn.Properties.Code).toEqual({
        S3Bucket: "aura-historia-release-bundles-eu-central-1",
        S3Key: { Ref: `${stem}ArtifactKey` },
        S3ObjectVersion: { Ref: `${stem}ArtifactVersion` },
      });
      const functionId = resourceByType(template, "AWS::Lambda::Function").find(([, resource]) => resource === fn)![0];
      const [[versionId, version]] = resourceByType(template, "AWS::Lambda::Version")
        .filter(([, resource]) => resource.Properties.FunctionName.Ref === functionId);
      expect(version.Properties.Description).toEqual({ Ref: `${stem}ArtifactSha256` });
      const [alias] = resourceByType(template, "AWS::Lambda::Alias")
        .filter(([, resource]) => resource.Properties.FunctionName.Ref === functionId).map(([, resource]) => resource);
      expect(alias.Properties).toMatchObject({ Name: "live", FunctionVersion: { "Fn::GetAtt": [versionId, "Version"] } });
    }
  });

  test("keeps the deployed pool, client, domain and provider identities and the exports the retired compute stack imports", () => {
    const template = identityTemplate(stage);
    const [[poolId, pool]] = resourceByType(template, "AWS::Cognito::UserPool");
    expect(poolId).toBe("IdentityPrimaryUserPool6ED02BC6");
    expect(pool).toMatchObject({ DeletionPolicy: "Retain", UpdateReplacePolicy: "Retain" });
    expect(pool.Properties.DeletionProtection).toBe("ACTIVE");
    expect(pool.Properties.UserPoolName).toBe(`primary-userpool-${stage}`);
    expect(Object.keys(template.toJSON().Outputs).filter((name) => name.startsWith("ExportsOutput")).sort()).toEqual([
      "ExportsOutputFnGetAttIdentityPrimaryUserPool6ED02BC6Arn798782F0",
      "ExportsOutputRefEmailConfigurationSet096C0FB57D524257",
      "ExportsOutputRefIdentityPrimaryUserPool6ED02BC6F7007C1B",
      "ExportsOutputRefIdentityPrimaryUserPoolPrimaryUserPoolClientPublic80694516DFB2D89A",
      "ExportsOutputRefIdentityPrimaryUserPoolPrimaryUserPoolCustomDomainCB5E75CB38985DE6",
    ]);
    expect(template.toJSON().Conditions).toBeUndefined();
  });
});

function rolePolicyStatements(template: Template, fn: Resource): Array<Record<string, unknown>> {
  const roleId = (fn.Properties.Role as { "Fn::GetAtt": [string, string] })["Fn::GetAtt"][0];
  return (Object.values(template.findResources("AWS::IAM::Policy")) as Resource[])
    .filter((policy) => (policy.Properties.Roles as Array<{ Ref: string }> | undefined)?.some((role) => role.Ref === roleId))
    .flatMap((policy) => policy.Properties.PolicyDocument.Statement as Array<Record<string, unknown>>);
}

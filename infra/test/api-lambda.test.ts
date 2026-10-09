import * as cdk from "aws-cdk-lib";
import { Template } from "aws-cdk-lib/assertions";
import { createApplicationStacks } from "../src/application-stack";
import { STAGES, type StageName } from "../src/config";
import { consentEvidenceLogGroupNames } from "../src/constructs/lambdas";

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

function apiAliasVersion(template: Template): { alias: CloudFormationResource; version: CloudFormationResource; versionId: string } {
  const aliases = Object.values(template.findResources("AWS::Lambda::Alias")) as CloudFormationResource[];
  expect(aliases).toHaveLength(1);
  const alias = aliases[0];
  const versionRef = alias.Properties.FunctionVersion as { "Fn::GetAtt": [string, string] };
  expect(versionRef["Fn::GetAtt"][1]).toBe("Version");
  const versionId = versionRef["Fn::GetAtt"][0];
  const version = template.findResources("AWS::Lambda::Version")[versionId] as CloudFormationResource | undefined;
  if (!version) {
    throw new Error(`API alias references missing version ${versionId}.`);
  }
  return { alias, version, versionId };
}

function resolveCommitSha(value: unknown, sha: string): string {
  if (typeof value === "string") {
    return value;
  }
  if (JSON.stringify(value) === JSON.stringify({ Ref: "CommitSHA" })) {
    return sha;
  }
  const join = (value as { "Fn::Join": [string, unknown[]] })["Fn::Join"];
  expect(join[0]).toBe("");
  return join[1].map((part) => resolveCommitSha(part, sha)).join("");
}

function expectedApiEnvironmentKeys(): string[] {
  const keys = [
    "AURA_HISTORIA_COGNITO_APP_CLIENT_IDS",
    "AURA_HISTORIA_COGNITO_ISSUER",
    "AURA_HISTORIA_COGNITO_JWKS_URL",
    "AURA_HISTORIA_COGNITO_USER_POOL_ID",
    "AURA_HISTORIA_GOOGLE_ADC_CREDENTIALS_JSON",
    "AWS_LAMBDA_HTTP_IGNORE_STAGE_IN_PATH",
    "COMMIT_SHA",
    "NEWSLETTER_CONFIRMATION_EMAIL_CONFIGURATION_SET",
    "NEWSLETTER_CONFIRMATION_EMAIL_FROM",
    "NEWSLETTER_CONFIRMATION_EMAIL_REPLY_TO",
    "NEWSLETTER_CONFIRMATION_FRONTEND_ORIGIN",
    "OPENSEARCH_ENDPOINT_URL",
    "PRODUCT_LISTING_INGESTION_QUEUE_URL",
    "POSTGRES_DATABASE",
    "POSTGRES_HOST",
    "POSTGRES_MAX_CONNECTIONS",
    "POSTGRES_PORT",
    "POSTGRES_TLS_ROOT_CERT",
    "S3_BUCKET_NAME_TEMPLATES",
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
    "LOOPS_API_BASE_URL",
    "LOOPS_API_KEY",
    "LOOPS_NEWSLETTER_LIST_ID",
    "LOOPS_WEBHOOK_SIGNING_SECRET",
  ];

  keys.push("OPENSEARCH_PASSWORD", "OPENSEARCH_USERNAME", "POSTGRES_SECRET_ARN");
  return keys.sort();
}

function expectedVertexEnvironment(stage: StageName): Record<string, string> {
  return {
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
    expect(environment.Variables.COMMIT_SHA).toEqual({ Ref: "CommitSHA" });
    expect(environment.Variables.S3_BUCKET_NAME_TEMPLATES).toBe("aura-historia-mail-templates-eu-central-1");
    expect(environment.Variables.NEWSLETTER_CONFIRMATION_EMAIL_FROM)
      .toBe(`{{resolve:ssm:/notifications/${stage}/email-from}}`);
    expect(environment.Variables.NEWSLETTER_CONFIRMATION_EMAIL_REPLY_TO)
      .toBe(`{{resolve:ssm:/notifications/${stage}/email-reply-to}}`);
    expect(environment.Variables.NEWSLETTER_CONFIRMATION_FRONTEND_ORIGIN).toBe(
      stage === "prod" ? "https://aura-historia.com" : "https://stage.aura-historia.com",
    );
    expect(environment.Variables.AWS_LAMBDA_HTTP_IGNORE_STAGE_IN_PATH).toBe("true");
    expect(environment.Variables.AURA_HISTORIA_COGNITO_ISSUER).toBeDefined();
    expect(environment.Variables.AURA_HISTORIA_COGNITO_JWKS_URL).toBeDefined();
    expect(environment.Variables.AURA_HISTORIA_COGNITO_APP_CLIENT_IDS).toBeDefined();
    expect(environment.Variables.AURA_HISTORIA_COGNITO_USER_POOL_ID).toBeDefined();
    expect(environment.Variables.OPENSEARCH_ENDPOINT_URL).toBeDefined();
    expect(environment.Variables.POSTGRES_MAX_CONNECTIONS).toBe("1");
    expect(environment.Variables.POSTGRES_SECRET_ARN).toBeDefined();
    expect(environment.Variables.POSTGRES_USERNAME).toBeUndefined();
    expect(environment.Variables.POSTGRES_PASSWORD).toBeUndefined();
    expect(Object.keys(environment.Variables).sort()).toEqual(expectedApiEnvironmentKeys());
    expect(environment.Variables).toMatchObject(expectedVertexEnvironment(stage));
    expect(environment.Variables.LOOPS_API_BASE_URL).toBe("https://app.loops.so/api");
    expect(environment.Variables.LOOPS_API_KEY).toBe(`{{resolve:ssm:/loops/${stage}/api-key}}`);
    expect(environment.Variables.LOOPS_NEWSLETTER_LIST_ID).toBe(`{{resolve:ssm:/loops/${stage}/newsletter-list-id}}`);
    expect(environment.Variables.LOOPS_WEBHOOK_SIGNING_SECRET).toBe(
      `{{resolve:ssm:/loops/${stage}/webhook-signing-secret}}`,
    );
    expect(JSON.stringify(functionResource.Properties).toLowerCase()).not.toContain("zoho");
    expect(environment.Variables.GOOGLE_APPLICATION_CREDENTIALS).toBeUndefined();
    expect(functionResource.Properties.ReservedConcurrentExecutions).toBeUndefined();
  });

  test("retains never-expiring log groups for each consent-emitting Lambda", () => {
    const template = computeTemplate(stage);
    const [apiLogGroupName, postConfirmationLogGroupName] = consentEvidenceLogGroupNames(stage);
    const logGroups = Object.entries(template.findResources("AWS::Logs::LogGroup")) as
      [string, CloudFormationResource & { DeletionPolicy?: string; UpdateReplacePolicy?: string }][];

    for (const [functionName, logGroupName] of [
      [`aura-historia-api-${stage}`, apiLogGroupName],
      [`cognito-post-confirmation-${stage}`, postConfirmationLogGroupName],
    ] as const) {
      const [logicalId, group] = logGroups.find(([, resource]) =>
        resource.Properties.LogGroupName === logGroupName,
      ) ?? [];
      expect(logicalId).toBeDefined();
      expect(group?.Properties.RetentionInDays).toBeUndefined();
      expect(group?.DeletionPolicy).toBe("Retain");
      expect(group?.UpdateReplacePolicy).toBe("Retain");

      const emitter = lambdaFunction(template, functionName);
      expect(emitter.Properties.LoggingConfig).toEqual({ LogGroup: { Ref: logicalId } });
    }

    const retentionLambda = lambdaFunction(template, `cloudwatch-log-retention-lambda-${stage}`);
    const retentionEnvironment = (retentionLambda.Properties.Environment as { Variables: Record<string, unknown> }).Variables;
    expect(retentionEnvironment.CONSENT_EVIDENCE_LOG_GROUPS).toBe(
      JSON.stringify([apiLogGroupName, postConfirmationLogGroupName]),
    );
    expect(retentionEnvironment.CONSENT_EVIDENCE_LOG_GROUPS).not.toContain("*");
  });

  test("keeps log administration limited to retention writes", () => {
    const template = computeTemplate(stage);
    const logStatements = Object.values(template.findResources("AWS::IAM::Policy"))
      .flatMap((resource) => (resource.Properties.PolicyDocument as { Statement: Record<string, unknown>[] }).Statement)
      .filter((statement) => JSON.stringify(statement.Action).includes("logs:"));

    expect(JSON.stringify(logStatements)).not.toMatch(/logs:(GetLogEvents|FilterLogEvents|DeleteLogGroup|DeleteLogStream)/);
    const putRetention = logStatements.find((statement) =>
      JSON.stringify(statement.Action).includes("logs:PutRetentionPolicy"),
    );
    expect(putRetention).toBeDefined();
    expect(putRetention?.Resource).not.toBe("*");
    expect(JSON.stringify(putRetention?.Resource)).toContain("log-group:*");
  });

  test("promotes parameter-only artifacts and rollback through an immutable live version", () => {
    const template = computeTemplate(stage);
    const { alias, version, versionId } = apiAliasVersion(template);
    const apiFunctionEntries = Object.entries(template.findResources("AWS::Lambda::Function"));
    const [functionId] = apiFunctionEntries.find(
      ([, resource]) => (resource as CloudFormationResource).Properties.FunctionName === `aura-historia-api-${stage}`,
    ) ?? [];
    const code = apiFunction(template, stage).Properties.Code as { S3Key: unknown };
    const firstSha = "aaaaaaaa";
    const nextSha = "bbbbbbbb";

    expect(alias.Properties).toMatchObject({ Name: "live", FunctionName: { Ref: functionId } });
    expect(version.Properties.FunctionName).toEqual({ Ref: functionId });
    expect(version.Properties.Description).toEqual({
      "Fn::Join": ["", ["aura-historia-api-", { Ref: "CommitSHA" }]],
    });
    expect(code.S3Key).toEqual({
      "Fn::Join": ["", [`aura-historia-api-${stage}-`, { Ref: "CommitSHA" }, ".zip"]],
    });
    // Same synthesized template: only the deploy-time parameter changes, never a synth-time clock or random ID.
    const [initial, promotion, rollback, unchanged] = [firstSha, nextSha, firstSha, firstSha]
      .map((sha) => resolveCommitSha(version.Properties.Description, sha));
    expect(initial).toBe("aura-historia-api-aaaaaaaa");
    expect(promotion).toBe("aura-historia-api-bbbbbbbb");
    expect(promotion).not.toBe(initial);
    expect(rollback).not.toBe(promotion);
    expect(rollback).toBe(initial);
    expect(unchanged).toBe(rollback);
    expect(resolveCommitSha(code.S3Key, firstSha)).toBe(`aura-historia-api-${stage}-aaaaaaaa.zip`);
    expect(resolveCommitSha(code.S3Key, nextSha)).toBe(`aura-historia-api-${stage}-bbbbbbbb.zip`);
    // Alias references the version resource, not $LATEST or any assumed numeric version ID.
    expect(alias.Properties.FunctionVersion).toEqual({ "Fn::GetAtt": [versionId, "Version"] });
    expect(version.Properties.ReservedConcurrentExecutions).toBeUndefined();
    expect(version.Properties.ProvisionedConcurrencyConfig).toBeUndefined();
  });

  test("limits Loops newsletter settings to the API and consent workers", () => {
    const template = computeTemplate(stage);
    const functions = Object.values(template.findResources("AWS::Lambda::Function")) as CloudFormationResource[];

    for (const functionResource of functions) {
      if ([`aura-historia-api-${stage}`, `marketing-consent-sync-lambda-${stage}`]
        .includes(functionResource.Properties.FunctionName as string)) {
        continue;
      }
      const environment = functionResource.Properties.Environment as
        | { Variables?: Record<string, unknown> }
        | undefined;
      const variables = environment?.Variables ?? {};
      expect(Object.keys(variables).filter((key) => key.startsWith("LOOPS_"))).toEqual([]);
      expect(JSON.stringify(functionResource.Properties)).not.toContain("/loops/");
      expect(JSON.stringify(functionResource.Properties)).not.toContain("loops.test");
    }
  });

  test("grants API only the newsletter templates and approved SES identity and configuration set", () => {
    const template = computeTemplate(stage);
    const api = apiFunction(template, stage);
    const roleId = (api.Properties.Role as { "Fn::GetAtt": [string, string] })["Fn::GetAtt"][0];
    const policies = Object.values(template.findResources("AWS::IAM::Policy")) as CloudFormationResource[];
    const apiStatements = policies
      .filter((policy) => (policy.Properties.Roles as Array<{ Ref: string }>).some((role) => role.Ref === roleId))
      .flatMap((policy) => (policy.Properties.PolicyDocument as { Statement: Record<string, unknown>[] }).Statement);
    const s3Statements = apiStatements.filter((statement) => JSON.stringify(statement.Action).includes("s3:"));
    const sesStatements = apiStatements.filter((statement) => JSON.stringify(statement.Action).includes("ses:"));

    expect(s3Statements).toHaveLength(1);
    expect(s3Statements[0].Action).toBe("s3:GetObject");
    expect(JSON.stringify(s3Statements[0].Resource)).toContain(
      `${stage}/`,
    );
    expect(JSON.stringify(s3Statements[0].Resource)).toContain('{"Ref":"CommitSHA"}');
    expect(JSON.stringify(s3Statements[0].Resource)).toContain(
      "/mjml/newsletter/confirmation/*",
    );
    expect(JSON.stringify(s3Statements[0].Resource)).not.toContain("/mjml/*");
    expect(JSON.stringify(s3Statements[0].Resource)).not.toContain("/newsletter/*");

    expect(sesStatements).toHaveLength(1);
    expect(sesStatements[0].Action).toBe("ses:SendEmail");
    expect(sesStatements[0].Resource).toHaveLength(2);
    expect(JSON.stringify(sesStatements[0].Resource)).toContain("identity/notify.aura-historia.com");
    const [configurationSetId] = Object.keys(template.findResources("AWS::SES::ConfigurationSet"));
    expect((sesStatements[0].Resource as unknown[])[1]).toEqual({
      "Fn::Join": ["", [
        "arn:", { Ref: "AWS::Partition" }, ":ses:", { Ref: "AWS::Region" }, ":",
        { Ref: "AWS::AccountId" }, ":configuration-set/", { Ref: configurationSetId },
      ]],
    });
    expect(JSON.stringify(sesStatements[0].Resource)).not.toContain("identity/*");
    expect(JSON.stringify(sesStatements[0].Resource)).not.toContain("configuration-set/*");
    expect(JSON.stringify(sesStatements[0].Action)).not.toContain("ses:*");
  });

  test("keeps Vertex ADC configuration and permissions out of the projector", () => {
    const template = computeTemplate(stage);
    for (const name of ["product-listing-opensearch-lambda", "search-filter-projection-lambda"]) {
      const projector = lambdaFunction(template, `${name}-${stage}`);
      const projectorEnvironment = projector.Properties.Environment as { Variables: Record<string, unknown> };

      expect(projectorEnvironment.Variables.AURA_HISTORIA_GOOGLE_ADC_CREDENTIALS_JSON).toBeUndefined();
      expect(projectorEnvironment.Variables.GOOGLE_APPLICATION_CREDENTIALS).toBeUndefined();
      expect(projectorEnvironment.Variables.VERTEX_AI_LOCATION).toBeUndefined();
      expect(projectorEnvironment.Variables.VERTEX_AI_PROJECT_ID).toBeUndefined();
      expect(JSON.stringify(projector.Properties)).not.toContain("google-application-credentials");
      expect(JSON.stringify(projector.Properties)).not.toContain("vertex-ai");
    }
    const projectorPolicies = Object.entries(template.findResources("AWS::IAM::Policy"))
      .filter(([id]) => id.includes("ProductListingOpenSearchLambda") || id.includes("SearchFilterProjectionLambda"));
    expect(JSON.stringify(projectorPolicies)).not.toContain("ssm:GetParameter");
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
    expect(functionResource.Properties.VpcConfig).toBeDefined();
    expect(JSON.stringify(eventSourceMappings)).not.toContain(apiFunctionLogicalId);
    expect(aliases).toHaveLength(1);
    expect(JSON.stringify(aliases[0])).toContain(apiFunctionLogicalId);
    expect(JSON.stringify(aliases[0])).toContain("live");
    expect(Object.values(template.findResources("AWS::Lambda::Url"))).toHaveLength(0);
    expect(Object.values(template.findResources("AWS::ApiGatewayV2::Integration"))).toHaveLength(0);
  });
});

describe.each(STAGES)("%s OpenSearch runtime credentials", (stage) => {
  test("uses distinct role-scoped SSM parameters for each runtime", () => {
    const template = computeTemplate(stage);
    const paths: unknown[] = [];

    for (const [binary, role] of [
      ["aura-historia-api", "reader"],
      ["product-listing-opensearch-lambda", "product-projector"],
      ["search-filter-projection-lambda", "filter-projector"],
      ["search-filter-percolator-lambda", "percolator"],
    ] as const) {
      const environment = lambdaFunction(template, `${binary}-${stage}`).Properties.Environment as {
        Variables: Record<string, unknown>;
      };
      expect(environment.Variables.OPENSEARCH_ENDPOINT_URL).toBeDefined();

      const username = `{{resolve:ssm:/opensearch/${stage}/${role}/username}}`;
      const password = `{{resolve:ssm:/opensearch/${stage}/${role}/password}}`;
      expect(environment.Variables.OPENSEARCH_USERNAME).toBe(username);
      expect(environment.Variables.OPENSEARCH_PASSWORD).toBe(password);
      paths.push(username, password);
    }

    expect(new Set(paths).size).toBe(8);
  });
});

test("configuration-only API updates still change the currentVersion target", () => {
  const before = computeTemplate("dev");
  const app = new cdk.App({ analyticsReporting: false });
  const stacks = createApplicationStacks(app, { stage: "dev" });
  stacks.compute.lambdas.functions.auraHistoriaApi.addEnvironment("TEST_VERSION_CONFIG", "changed");
  const afterTemplate = Template.fromStack(stacks.compute);
  const { alias, versionId: after } = apiAliasVersion(afterTemplate);

  expect(after).not.toBe(apiAliasVersion(before).versionId);
  expect(alias.Properties.FunctionVersion).toEqual({ "Fn::GetAtt": [after, "Version"] });
  expect(afterTemplate.findResources("AWS::Lambda::Version")[after].Properties.Description).toEqual({
    "Fn::Join": ["", ["aura-historia-api-", { Ref: "CommitSHA" }]],
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

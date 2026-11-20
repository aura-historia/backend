import * as cdk from "aws-cdk-lib";
import { Template } from "aws-cdk-lib/assertions";
import { createApplicationStacks } from "../src/application-stack";
import { stageConfig } from "../src/config";

type Statement = {
  readonly Effect: "Allow" | "Deny";
  readonly Action: string | string[];
  readonly Resource: unknown;
  readonly Condition?: Record<string, Record<string, string>>;
  readonly Principal?: unknown;
};
type Resource = { readonly Properties: Record<string, any>; readonly DeletionPolicy?: string };

const config = stageConfig("prod");
const stacks = createApplicationStacks(new cdk.App({ analyticsReporting: false }), {
  stage: "prod",
  env: { account: "123456789012", region: "eu-central-1" },
});
const host = Template.fromStack(stacks.host);
const identity = Template.fromStack(stacks.identity);

function only(template: Template, type: string): [string, Resource] {
  const entries = Object.entries(template.findResources(type)) as [string, Resource][];
  expect(entries).toHaveLength(1);
  return entries[0];
}

const actions = (statement: Statement) => [statement.Action].flat();

/** Every statement the host user is allowed, from the host stack and the identity stack. */
function hostUserStatements(): Statement[] {
  return [host, identity].flatMap((template) => Object.values(template.findResources("AWS::IAM::ManagedPolicy")) as Resource[])
    .filter((policy) => JSON.stringify(policy.Properties.Users ?? []).match(/HostUser|aura-historia-prod-host/))
    .flatMap((policy) => policy.Properties.PolicyDocument.Statement as Statement[]);
}

function allowed(action: string): Statement[] {
  return hostUserStatements().filter((statement) => statement.Effect === "Allow" && actions(statement).includes(action));
}

describe("host access stack", () => {
  test("creates one IAM user and no access key, so no credential passes through CloudFormation", () => {
    const [, user] = only(host, "AWS::IAM::User");
    expect(user.Properties.UserName).toBe("aura-historia-prod-host");
    expect(user.Properties.Policies).toBeUndefined();
    // Grants are managed policies, leaving IAM's 2,048-character aggregate inline limit unused.
    for (const template of [host, identity]) {
      const inline = Object.values(template.findResources("AWS::IAM::Policy")) as Resource[];
      expect(inline.filter((policy) => JSON.stringify(policy.Properties.Users ?? []).match(/HostUser|aura-historia-prod-host/))).toEqual([]);
    }
    expect(host.findResources("AWS::IAM::AccessKey")).toEqual({});
    // The only role is the trigger artifact publisher, which GitHub assumes, never the host.
    expect(Object.values(host.findResources("AWS::IAM::Role")).map((role: any) => role.Properties.RoleName))
      .toEqual(["aura-historia-prod-trigger-artifact-publisher"]);
  });

  test("keeps every backup version: versioned, TLS-only, private and retained with its stack", () => {
    const [bucketId, bucket] = only(host, "AWS::S3::Bucket");
    expect(bucket.DeletionPolicy).toBe("Retain");
    expect(bucket.Properties).toMatchObject({
      BucketName: "aura-historia-prod-backups-eu-central-1",
      VersioningConfiguration: { Status: "Enabled" },
      PublicAccessBlockConfiguration: {
        BlockPublicAcls: true, BlockPublicPolicy: true, IgnorePublicAcls: true, RestrictPublicBuckets: true,
      },
      OwnershipControls: { Rules: [{ ObjectOwnership: "BucketOwnerEnforced" }] },
      BucketEncryption: { ServerSideEncryptionConfiguration: [{ ServerSideEncryptionByDefault: { SSEAlgorithm: "AES256" } }] },
    });
    expect(bucket.Properties.LifecycleConfiguration.Rules).toEqual([{
      Id: "ExpireReplacedVersions",
      Status: "Enabled",
      NoncurrentVersionExpiration: { NoncurrentDays: config.host.backupNoncurrentVersionRetentionDays },
      ExpiredObjectDeleteMarker: true,
      AbortIncompleteMultipartUpload: { DaysAfterInitiation: 7 },
    }]);
    // Old versions must outlive pgBackRest's 14 days of retained backups.
    expect(config.host.backupNoncurrentVersionRetentionDays).toBeGreaterThan(14);

    const [, policy] = only(host, "AWS::S3::BucketPolicy");
    expect(policy.Properties.Bucket).toEqual({ Ref: bucketId });
    const statements = policy.Properties.PolicyDocument.Statement as Statement[];
    const tls = statements.find((statement) => JSON.stringify(statement.Condition ?? {}).includes("aws:SecureTransport"));
    expect(tls).toMatchObject({ Effect: "Deny", Action: "s3:*", Condition: { Bool: { "aws:SecureTransport": "false" } } });
    const minimumTls = statements.find((statement) => JSON.stringify(statement.Condition ?? {}).includes("s3:TlsVersion"));
    expect(minimumTls).toMatchObject({ Effect: "Deny", Condition: { NumericLessThan: { "s3:TlsVersion": 1.2 } } });
  });

  test("denies the host user removing old versions or the settings that keep them, in the bucket policy itself", () => {
    const [, policy] = only(host, "AWS::S3::BucketPolicy");
    const [userId] = only(host, "AWS::IAM::User");
    const deny = (policy.Properties.PolicyDocument.Statement as (Statement & { Sid?: string })[])
      .find((statement) => statement.Sid === "DenyHostVersionRemoval");
    expect(deny?.Effect).toBe("Deny");
    expect(deny?.Principal).toEqual({ AWS: { "Fn::GetAtt": [userId, "Arn"] } });
    expect(actions(deny!).sort()).toEqual([
      "s3:DeleteBucket", "s3:DeleteBucketPolicy", "s3:DeleteObjectVersion",
      "s3:PutBucketPolicy", "s3:PutBucketVersioning", "s3:PutLifecycleConfiguration",
    ]);
  });

  test("lets the host read, write and expire current backup objects under its two prefixes only", () => {
    const s3Statements = hostUserStatements().filter((statement) => actions(statement).some((action) => action.startsWith("s3:")));
    expect(s3Statements.map((statement) => actions(statement))).toEqual([
      ["s3:ListBucket", "s3:GetBucketLocation"],
      ["s3:GetObject", "s3:PutObject", "s3:DeleteObject"],
    ]);
    expect(JSON.stringify(s3Statements[1].Resource)).toMatch(/\/pgbackrest\/\*.*\/secrets\/\*/);
    expect(JSON.stringify(hostUserStatements())).not.toMatch(/s3:\*|s3:DeleteObjectVersion|s3:PutBucket|s3:PutLifecycle/);
  });

  test("sends email only through the managed configuration set and the notification identity", () => {
    const [send] = allowed("ses:SendEmail");
    expect(actions(send)).toEqual(["ses:SendEmail"]);
    const resources = JSON.stringify(send.Resource);
    expect(resources).toContain(":identity/notify.aura-historia.com");
    expect(resources).toContain(":configuration-set/aura-historia-prod-email");
    expect(resources).not.toContain("*");
  });

  test("calls exactly the Amazon Location operations of the REST API and the geographic worker, in the workload region", () => {
    const [places] = allowed("geo-places:Geocode");
    expect(actions(places).sort()).toEqual(["geo-places:Autocomplete", "geo-places:Geocode", "geo-places:GetPlace"]);
    expect(places.Condition).toEqual({ StringEquals: { "aws:RequestedRegion": "eu-central-1" } });
  });

  test("grants today's Cognito admin calls on the production pool alone, from the pool's own stack", () => {
    const [poolId] = only(identity, "AWS::Cognito::UserPool");
    const cognito = hostUserStatements().filter((statement) => actions(statement).some((action) => action.startsWith("cognito-idp:")));
    expect(cognito).toHaveLength(1);
    expect(actions(cognito[0]).sort()).toEqual([
      "cognito-idp:AdminDeleteUser", "cognito-idp:AdminGetUser", "cognito-idp:AdminUserGlobalSignOut", "cognito-idp:ListUsers",
    ]);
    expect(cognito[0].Resource).toEqual({ "Fn::GetAtt": [poolId, "Arn"] });
    const [, administration] = Object.entries(identity.findResources("AWS::IAM::ManagedPolicy"))
      .find(([id]) => id.startsWith("HostCognitoAdministration")) as [string, Resource];
    expect(administration.Properties.Users).toEqual(["aura-historia-prod-host"]);
  });

  test("writes only to the consent-evidence and container log groups and to one metric namespace", () => {
    const [logs] = allowed("logs:PutLogEvents");
    expect(actions(logs)).toEqual(["logs:CreateLogStream", "logs:PutLogEvents"]);
    const resources = JSON.stringify(logs.Resource);
    for (const name of [
      "log-group:/aws/lambda/aura-historia-api-prod:*",
      "log-group:/aws/lambda/cognito-post-confirmation-prod:*",
      "log-group:/aura-historia/prod/host/*",
    ]) expect(resources).toContain(name);
    expect((logs.Resource as unknown[]).length).toBe(3);
    expect(JSON.stringify(hostUserStatements())).not.toMatch(/logs:(CreateLogGroup|PutRetentionPolicy|DeleteLogGroup|\*)/);

    const [metrics] = allowed("cloudwatch:PutMetricData");
    expect(actions(metrics)).toEqual(["cloudwatch:PutMetricData"]);
    expect(metrics.Condition).toEqual({ StringEquals: { "cloudwatch:namespace": "AuraHistoria/Host" } });
  });

  test("allows nothing beyond those calls", () => {
    expect(hostUserStatements().every((statement) => statement.Effect === "Allow")).toBe(true);
    expect(hostUserStatements().flatMap(actions).sort()).toEqual([
      "cloudwatch:PutMetricData",
      "cognito-idp:AdminDeleteUser", "cognito-idp:AdminGetUser", "cognito-idp:AdminUserGlobalSignOut", "cognito-idp:ListUsers",
      "geo-places:Autocomplete", "geo-places:Geocode", "geo-places:GetPlace",
      "logs:CreateLogStream", "logs:PutLogEvents",
      "s3:DeleteObject", "s3:GetBucketLocation", "s3:GetObject", "s3:ListBucket", "s3:PutObject",
      "ses:SendEmail",
    ]);
  });

  test("generates the retained origin secret and Cognito trigger signing key that the host requires", () => {
    const secrets = Object.values(host.findResources("AWS::SecretsManager::Secret")) as Resource[];
    expect(secrets.map((secret) => secret.Properties.Name).sort()).toEqual([
      "/aura-historia/prod/edge/origin-verify",
      "/aura-historia/prod/edge/post-confirmation-signing",
    ]);
    for (const secret of secrets) {
      expect(secret.DeletionPolicy).toBe("Retain");
      expect(secret.Properties.GenerateSecretString).toEqual({
        SecretStringTemplate: "{}", GenerateStringKey: "value", PasswordLength: 64, ExcludePunctuation: true,
      });
      expect(secret.Properties.SecretString).toBeUndefined();
    }
    // The host user never reads them; an operator installs them in the host's secret files.
    expect(JSON.stringify(hostUserStatements())).not.toContain("secretsmanager:");
  });
});

describe("trigger artifact publisher", () => {
  const [, role] = only(host, "AWS::IAM::Role");
  const policies = Object.values(host.findResources("AWS::IAM::Policy")) as Resource[];
  const statements = policies.flatMap((policy) => policy.Properties.PolicyDocument.Statement as Statement[]);

  test("is assumable only by the Deploy workflow's prod environment through GitHub's OIDC provider", () => {
    const [trust] = role.Properties.AssumeRolePolicyDocument.Statement as Statement[];
    expect(trust.Action).toBe("sts:AssumeRoleWithWebIdentity");
    expect(JSON.stringify(trust.Principal)).toContain("oidc-provider/token.actions.githubusercontent.com");
    expect(trust.Condition).toEqual({
      StringEquals: {
        "token.actions.githubusercontent.com:aud": "sts.amazonaws.com",
        "token.actions.githubusercontent.com:sub": "repo:aura-historia/backend:environment:prod",
      },
    });
  });

  test("may only create trigger zips in the artifact bucket", () => {
    expect(statements).toHaveLength(1);
    expect(actions(statements[0])).toEqual(["s3:PutObject"]);
    expect(JSON.stringify(statements[0].Resource)).toContain(":s3:::aura-historia-binary-artifacts-eu-central-1/cognito-*-prod-*.zip");
    expect(statements[0].Condition).toEqual({ StringEquals: { "s3:if-none-match": "*" } });
  });
});

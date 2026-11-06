import * as cdk from "aws-cdk-lib";
import type { Construct } from "constructs";
import type { StageName } from "./config";

/**
 * Template metadata naming the secret each credential version parameter pins. The release planner
 * (ci/delivery/credential-versions.cjs) binds every listed parameter to that secret's AWSCURRENT
 * version, so changing a credential is an explicit parameter change on the next release.
 */
export const CREDENTIAL_VERSIONS_METADATA = "AuraHistoria::CredentialVersions";
// Secrets Manager generates UUID version IDs; a custom client request token would not fit.
const SECRET_VERSION_ID_PATTERN = "^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$";
// The random final UUID segment (48 bits) identifies a version in a Lambda version description.
const VERSION_DESCRIPTION_SEGMENT_LENGTH = 12;
const LAMBDA_VERSION_DESCRIPTION_LIMIT = 256;

/** A Secrets Manager secret whose JSON fields are injected into configuration at deployment. */
export interface CredentialSecret<Field extends string = string> {
  /** Stem of the stack parameter `<stem>CredentialVersion` that pins the secret version. */
  readonly parameterStem: string;
  readonly secretName: string;
  readonly fields: readonly Field[];
}

function secret<const Field extends string>(
  parameterStem: string,
  secretName: string,
  fields: readonly Field[],
): CredentialSecret<Field> {
  return { parameterStem, secretName, fields };
}

/**
 * Runtime credentials. Operators provision the app secrets as JSON objects with exactly these
 * fields before a release references them; the runtime PostgreSQL secret is generated with the
 * database. Database administration, migration, inspection and replication secrets are read
 * only by the database operations and DMS, never injected into runtime configuration.
 */
export function credentialSecrets(stage: StageName) {
  const app = (name: string) => `/aura-historia/${stage}/app/${name}`;
  return {
    postgresRuntime: secret("PostgresRuntime", `/aura-historia/${stage}/postgres/runtime`, ["username", "password"]),
    stripe: secret("Stripe", app("stripe"), ["apiKey"]),
    loops: secret("Loops", app("loops"), ["apiKey", "webhookSigningSecret"]),
    openSearch: secret("OpenSearch", app("opensearch"), ["readerPassword", "productProjectorPassword"]),
    anthropic: secret("Anthropic", app("anthropic"), ["apiKey"]),
    google: secret("Google", app("google"), ["applicationCredentials"]),
    cloudflare: secret("Cloudflare", app("cloudflare"), ["workersAiApiToken"]),
    fxRatesApi: secret("FxRatesApi", app("fxratesapi"), ["apiToken"]),
    cognitoIdentityProviders: secret("CognitoIdentityProviders", app("cognito-identity-providers"),
      ["googleClientSecret", "facebookClientSecret"]),
  } as const;
}

/** Resolves one field of `credential` at deployment, pinned to the stack's version parameter. */
export type CredentialResolver = <Field extends string>(credential: CredentialSecret<Field>, field: Field) => string;

export function credentialResolver(scope: Construct): CredentialResolver {
  return (credential, field) => credentialValue(scope, credential, field).unsafeUnwrap();
}

/** A resolver that also records which secrets one function's configuration uses. */
export function recordingCredentialResolver(scope: Construct): {
  readonly resolve: CredentialResolver;
  readonly used: () => readonly CredentialSecret[];
} {
  const resolve = credentialResolver(scope);
  const used: CredentialSecret[] = [];
  return {
    resolve: (credential, field) => {
      used.push(credential);
      return resolve(credential, field);
    },
    used: () => used,
  };
}

/**
 * A published Lambda version snapshots its configuration, so its identity must change with the
 * pinned versions of the credentials it uses as well as with its code; otherwise a release that
 * changes only a credential would leave aliases and versioned consumers on the old value.
 */
export function releaseVersionDescription(
  scope: Construct,
  artifactSha256: string,
  credentials: readonly CredentialSecret[],
): string {
  const pinned = [...new Map(credentials.map((credential) => [credential.parameterStem, credential])).values()]
    .sort((left, right) => left.parameterStem.localeCompare(right.parameterStem));
  if (pinned.length === 0) {
    return artifactSha256;
  }
  if (64 + pinned.length * (VERSION_DESCRIPTION_SEGMENT_LENGTH + 1) > LAMBDA_VERSION_DESCRIPTION_LIMIT) {
    throw new Error(`A Lambda version cannot pin ${pinned.length} credential secrets in its description.`);
  }
  const stack = cdk.Stack.of(scope);
  return cdk.Fn.join(" ", [artifactSha256, ...pinned.map((credential) =>
    cdk.Fn.select(4, cdk.Fn.split("-", versionParameter(stack, credential).valueAsString)))]);
}

/**
 * CloudFormation resolves the value when it creates or updates the resource. Change sets show it as
 * `{{changeSet:SENSITIVE}}`; the deployed resource configuration holds the plaintext.
 */
export function credentialValue<Field extends string>(
  scope: Construct,
  credential: CredentialSecret<Field>,
  field: Field,
): cdk.SecretValue {
  if (!credential.fields.includes(field)) {
    throw new Error(`${credential.secretName} has no field ${field}.`);
  }
  return cdk.SecretValue.secretsManager(credential.secretName, {
    jsonField: field,
    versionId: versionParameter(cdk.Stack.of(scope), credential).valueAsString,
  });
}

function versionParameter(stack: cdk.Stack, credential: CredentialSecret): cdk.CfnParameter {
  const id = `${credential.parameterStem}CredentialVersion`;
  const existing = stack.node.tryFindChild(id);
  if (existing instanceof cdk.CfnParameter) {
    return existing;
  }
  if (existing) {
    throw new Error(`${id} is not a credential version parameter.`);
  }
  const parameter = new cdk.CfnParameter(stack, id, {
    type: "String",
    allowedPattern: SECRET_VERSION_ID_PATTERN,
    description: `Secrets Manager version of ${credential.secretName}; bound at release`,
  });
  const metadata = stack.templateOptions.metadata ?? {};
  stack.templateOptions.metadata = {
    ...metadata,
    [CREDENTIAL_VERSIONS_METADATA]: { ...(metadata[CREDENTIAL_VERSIONS_METADATA] ?? {}), [id]: credential.secretName },
  };
  return parameter;
}

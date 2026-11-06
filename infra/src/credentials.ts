import * as cdk from "aws-cdk-lib";
import type { Construct } from "constructs";
import type { StageName } from "./config";

/**
 * Template metadata naming the secret each credential version parameter pins. The release planner
 * (ci/delivery/credential-versions.cjs) binds every listed parameter to that secret's AWSCURRENT
 * version, so changing a credential is an explicit parameter change on the next release.
 */
export const CREDENTIAL_VERSIONS_METADATA = "AuraHistoria::CredentialVersions";
const SECRET_VERSION_ID_PATTERN = "^[A-Za-z0-9-]{32,64}$";

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

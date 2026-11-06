import type { CredentialSecret } from "../src/credentials";

/** The synthesized deploy-time reference to one field of a version-pinned credential secret. */
export function credentialReference<Field extends string>(secret: CredentialSecret<Field>, field: Field): unknown {
  return {
    "Fn::Join": ["", [
      `{{resolve:secretsmanager:${secret.secretName}:SecretString:${field}::`,
      { Ref: `${secret.parameterStem}CredentialVersion` },
      "}}",
    ]],
  };
}

/** The synthesized description that ties a Lambda version to its code and pinned credentials. */
export function releaseVersionDescription(artifactSha256Parameter: string, secrets: readonly CredentialSecret[]): unknown {
  const pinned = [...secrets].sort((left, right) => left.parameterStem.localeCompare(right.parameterStem));
  return {
    "Fn::Join": [" ", [
      { Ref: artifactSha256Parameter },
      ...pinned.map((secret) => ({ "Fn::Select": [4, { "Fn::Split": ["-", { Ref: `${secret.parameterStem}CredentialVersion` }] }] })),
    ]],
  };
}

/** Transitional (#2798): the runtime secret grant kept until the deploy-time credential cutover completes. */
export function transitionalRuntimeSecretGrant(stage: string): unknown {
  return {
    Action: "secretsmanager:GetSecretValue",
    Effect: "Allow",
    Resource: { "Fn::ImportValue": `application-${stage}-data:ExportsOutputRefStoragePostgresRuntimeCredentials3F505EE124FDDA1A` },
  };
}

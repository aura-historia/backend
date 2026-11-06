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

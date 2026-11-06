"use strict";
// Templates pin Secrets Manager references to a version parameter and declare, in this metadata
// key, which secret each parameter tracks. Routine delivery binds every parameter to the secret's
// current version, so a changed credential becomes an explicit, reviewable parameter change.
const CREDENTIAL_VERSIONS_METADATA = "AuraHistoria::CredentialVersions";
const PARAMETER = /^[A-Z][A-Za-z0-9]*CredentialVersion$/;
const VERSION_ID = /^[A-Za-z0-9-]{32,64}$/;

function declaredCredentialVersions(target, stackName, template) {
  const declared = template?.Metadata?.[CREDENTIAL_VERSIONS_METADATA];
  if (declared === undefined) return [];
  if (declared === null || typeof declared !== "object" || Array.isArray(declared))
    throw new Error(`Invalid credential version metadata in ${stackName}`);
  const secretId = new RegExp(
    `^/aura-historia/${target}/(postgres/runtime|app/[a-z0-9-]+)$`,
  );
  return Object.entries(declared).map(([parameter, secret]) => {
    if (
      !PARAMETER.test(parameter) ||
      !Object.hasOwn(template.Parameters || {}, parameter) ||
      typeof secret !== "string" ||
      !secretId.test(secret)
    )
      throw new Error(
        `Credential version ${parameter} in ${stackName} must pin a declared parameter to a ${target} runtime or app secret`,
      );
    return { parameter, secret };
  });
}

function currentVersion(ctx, secret, api) {
  const described = api(ctx, "secretsmanager", "describe-secret", [
    "--secret-id",
    secret,
  ]);
  const current = Object.entries(described.VersionIdsToStages || {}).filter(
    ([, stages]) => Array.isArray(stages) && stages.includes("AWSCURRENT"),
  );
  if (
    described.DeletedDate ||
    current.length !== 1 ||
    !VERSION_ID.test(current[0][0])
  )
    throw new Error(
      `Secret ${secret} has no single current version; provision it before delivery`,
    );
  return current[0][0];
}

/** Parameters the template binds to secret versions; delivery sets them, never preserves them. */
function credentialVersionParameterNames(template) {
  return Object.keys(template?.Metadata?.[CREDENTIAL_VERSIONS_METADATA] || {});
}

/** Resolves every declared credential version parameter to its secret's AWSCURRENT version. */
function credentialVersionParameters(ctx, templates, api) {
  const secrets = new Map();
  for (const [stackName, template] of Object.entries(templates))
    for (const { parameter, secret } of declaredCredentialVersions(
      ctx.target,
      stackName,
      template,
    )) {
      if (secrets.has(parameter) && secrets.get(parameter) !== secret)
        throw new Error(
          `Credential version ${parameter} pins different secrets across stacks`,
        );
      secrets.set(parameter, secret);
    }
  const versions = new Map();
  const values = {};
  for (const [parameter, secret] of secrets) {
    if (!versions.has(secret)) versions.set(secret, currentVersion(ctx, secret, api));
    values[parameter] = versions.get(secret);
  }
  return values;
}

module.exports = {
  CREDENTIAL_VERSIONS_METADATA,
  credentialVersionParameterNames,
  credentialVersionParameters,
};

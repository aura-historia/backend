"use strict";
// Templates pin Secrets Manager references to a version parameter and declare, in this metadata
// key, which secret each parameter tracks. Routine delivery binds every parameter to the secret's
// current version, so a changed credential becomes an explicit, reviewable parameter change.
const CREDENTIAL_VERSIONS_METADATA = "AuraHistoria::CredentialVersions";
const PARAMETER = /^[A-Z][A-Za-z0-9]*CredentialVersion$/;
// Secrets Manager generates UUID version IDs; templates accept nothing else (infra/src/credentials.ts).
const VERSION_ID = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/;

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
  let described;
  try {
    described = api(ctx, "secretsmanager", "describe-secret", ["--secret-id", secret]);
  } catch (cause) {
    throw new Error(`Secret ${secret} is not describable; provision it before delivery`, { cause });
  }
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

// Platform procedure for stacks routine delivery does not plan (identity, initialize):
//   node ci/delivery/credential-versions.cjs <dev|prod> <template.json>...
// prints the change-set parameters that bind each declared credential version, metadata only.
if (require.main === module) {
  const [target, ...files] = process.argv.slice(2);
  if (!["dev", "prod"].includes(target) || !files.length) {
    console.error("usage: credential-versions.cjs <dev|prod> <template.json>...");
    process.exit(2);
  }
  const fs = require("node:fs");
  const { aws } = require("./admission.cjs");
  const templates = Object.fromEntries(
    files.map((file) => [file, JSON.parse(fs.readFileSync(file, "utf8"))]),
  );
  const values = credentialVersionParameters(
    { target, region: "eu-central-1" },
    templates,
    aws,
  );
  console.log(
    JSON.stringify(
      Object.entries(values).map(([ParameterKey, ParameterValue]) => ({
        ParameterKey,
        ParameterValue,
      })),
      null,
      2,
    ),
  );
}

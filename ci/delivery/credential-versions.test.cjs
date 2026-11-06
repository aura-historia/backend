"use strict";
const { test } = require("node:test");
const assert = require("node:assert/strict");
const {
  CREDENTIAL_VERSIONS_METADATA,
  credentialVersionParameters,
} = require("./credential-versions.cjs");

const ctx = { target: "prod" };
const CURRENT = "a1b2c3d4-0000-4000-8000-000000000001";
const PREVIOUS = "a1b2c3d4-0000-4000-8000-000000000002";
const template = (declared) => ({
  Parameters: Object.fromEntries(
    Object.keys(declared).map((name) => [name, { Type: "String" }]),
  ),
  Metadata: { [CREDENTIAL_VERSIONS_METADATA]: declared },
  Resources: {},
});
const api = (described) => {
  const calls = [];
  const call = (_ctx, service, operation, args) => {
    assert.equal(`${service}/${operation}`, "secretsmanager/describe-secret");
    const secret = args[args.indexOf("--secret-id") + 1];
    calls.push(secret);
    return described[secret];
  };
  call.calls = calls;
  return call;
};

test("each declared parameter is bound to its secret's current version, read once per secret", () => {
  const read = api({
    "/aura-historia/prod/postgres/runtime": {
      VersionIdsToStages: { [CURRENT]: ["AWSCURRENT"], [PREVIOUS]: ["AWSPREVIOUS"] },
    },
  });
  const values = credentialVersionParameters(
    ctx,
    {
      "application-prod-compute": template({
        PostgresRuntimeCredentialVersion: "/aura-historia/prod/postgres/runtime",
      }),
      "application-prod-api": template({
        PostgresRuntimeCredentialVersion: "/aura-historia/prod/postgres/runtime",
      }),
      "application-prod-observability": { Resources: {} },
    },
    read,
  );
  assert.deepEqual(values, { PostgresRuntimeCredentialVersion: CURRENT });
  assert.deepEqual(read.calls, ["/aura-historia/prod/postgres/runtime"]);
});

test("undeclared parameters, foreign or conflicting secrets and missing versions fail closed", () => {
  const read = api({
    "/aura-historia/prod/app/stripe": { VersionIdsToStages: { [PREVIOUS]: ["AWSPREVIOUS"] } },
    "/aura-historia/prod/app/loops": {
      DeletedDate: "2026-11-06T00:00:00Z",
      VersionIdsToStages: { [CURRENT]: ["AWSCURRENT"] },
    },
  });
  const reject = (templates, pattern) =>
    assert.throws(() => credentialVersionParameters(ctx, templates, read), pattern);
  reject(
    {
      compute: {
        Parameters: {},
        Metadata: { [CREDENTIAL_VERSIONS_METADATA]: { StripeCredentialVersion: "/aura-historia/prod/app/stripe" } },
      },
    },
    /declared parameter/,
  );
  reject({ compute: template({ StripeCredentialVersion: "/aura-historia/dev/app/stripe" }) }, /prod runtime or app secret/);
  reject({ compute: template({ StripeCredentialVersion: "/aura-historia/prod/postgres/admin" }) }, /prod runtime or app secret/);
  reject({ compute: template({ Stripe: "/aura-historia/prod/app/stripe" }) }, /declared parameter/);
  reject(
    {
      compute: template({ StripeCredentialVersion: "/aura-historia/prod/app/stripe" }),
      api: template({ StripeCredentialVersion: "/aura-historia/prod/app/loops" }),
    },
    /different secrets/,
  );
  reject({ compute: template({ StripeCredentialVersion: "/aura-historia/prod/app/stripe" }) }, /no single current version/);
  reject({ compute: template({ LoopsCredentialVersion: "/aura-historia/prod/app/loops" }) }, /no single current version/);
  reject({ compute: { Metadata: { [CREDENTIAL_VERSIONS_METADATA]: [] } } }, /Invalid credential version metadata/);
});

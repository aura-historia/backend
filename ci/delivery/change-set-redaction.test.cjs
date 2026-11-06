"use strict";
const { test } = require("node:test");
const assert = require("node:assert/strict");
const { REDACTED, redactResolvedValues } = require("./change-set-redaction.cjs");

const template = (variables) => ({
  Resources: {
    Worker: {
      Type: "AWS::Lambda::Function",
      Properties: { MemorySize: 512, Environment: { Variables: variables } },
    },
  },
});
const before = template({
  STAGE: "prod",
  STRIPE_API_KEY: "{{resolve:ssm:/stripe/prod/api-key}}",
});
const after = template({
  STAGE: "prod",
  STRIPE_API_KEY: {
    "Fn::Join": [
      "",
      [
        "{{resolve:secretsmanager:/aura-historia/prod/app/stripe:SecretString:apiKey::",
        { Ref: "StripeCredentialVersion" },
        "}}",
      ],
    ],
  },
});
const context = (key) =>
  JSON.stringify({
    Properties: {
      MemorySize: "512",
      Environment: { Variables: { STAGE: "prod", STRIPE_API_KEY: key } },
    },
  });
const change = (resource) => ({ ResourceChange: { LogicalResourceId: "Worker", ...resource } });
const variable = (name, beforeValue, afterValue) => ({
  Target: {
    Attribute: "Properties",
    Name: "Environment",
    Path: `/Properties/Environment/Variables/${name}`,
    BeforeValue: beforeValue,
    AfterValue: afterValue,
  },
});

test("resolved reference values are removed from contexts and details while literals stay reviewable", () => {
  const [redacted] = redactResolvedValues(
    [
      change({
        BeforeContext: context("sk_live_old"),
        AfterContext: context("{{changeSet:SENSITIVE}}"),
        Details: [
          variable("STRIPE_API_KEY", "sk_live_old", "{{changeSet:SENSITIVE}}"),
          variable("STAGE", "dev", "prod"),
        ],
      }),
    ],
    before,
    after,
  );
  const serialized = JSON.stringify(redacted);
  assert.ok(!serialized.includes("sk_live_old"));
  const beforeContext = JSON.parse(redacted.ResourceChange.BeforeContext).Properties;
  assert.equal(beforeContext.Environment.Variables.STRIPE_API_KEY, REDACTED);
  assert.equal(beforeContext.Environment.Variables.STAGE, "prod");
  assert.equal(beforeContext.MemorySize, "512");
  assert.equal(redacted.ResourceChange.Details[0].Target.BeforeValue, REDACTED);
  assert.equal(redacted.ResourceChange.Details[0].Target.AfterValue, REDACTED);
  assert.deepEqual(redacted.ResourceChange.Details[1].Target.AfterValue, "prod");
});

test("unknown resources and unparseable contexts fail closed", () => {
  const [unknown, broken] = redactResolvedValues(
    [
      { ResourceChange: { LogicalResourceId: "Removed", BeforeContext: context("secret") } },
      change({ AfterContext: "{not json" }),
    ],
    before,
    after,
  );
  assert.ok(!unknown.ResourceChange.BeforeContext.includes("secret"));
  assert.equal(broken.ResourceChange.AfterContext, REDACTED);
});

test("redaction is deterministic so approval and execution compare the same evidence", () => {
  const changes = [
    change({
      BeforeContext: context("sk_live_old"),
      Details: [variable("STRIPE_API_KEY", "sk_live_old", "new")],
    }),
  ];
  assert.deepEqual(
    redactResolvedValues(changes, before, after),
    redactResolvedValues(structuredClone(changes), before, after),
  );
});

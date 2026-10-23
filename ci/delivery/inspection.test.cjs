"use strict";
const { test } = require("node:test");
const assert = require("node:assert/strict");
const { createHash } = require("node:crypto");
const {
  ledgerFindings,
  schemaFindings,
  resourceFindings,
  revalidate,
  prepareChangeSet,
} = require("./inspection.cjs");
const { snapshots } = require("./schema.cjs");
const { digest } = require("./contracts.cjs");

test("ledger checksum/dirty/unknown findings and captured schema changes cannot enter routine", () => {
  assert.deepEqual(
    ledgerFindings(
      [{ version: 1, checksum: "a", success: true }],
      [{ version: 1, checksum: "a" }],
    ),
    [],
  );
  assert.deepEqual(
    ledgerFindings(
      [
        { version: 1, checksum: "b", success: false },
        { version: 2, checksum: "a", success: true },
      ],
      [{ version: 1, checksum: "a" }],
    ).map((item) => item.code),
    ["DIRTY_MIGRATION", "APPLIED_CHECKSUM_DRIFT", "UNKNOWN_MIGRATION"],
  );
  const before = [{ relation: "public.captured", columns: ["id"] }],
    after = [{ relation: "public.captured", columns: ["id", "private_email"] }];
  assert.equal(
    schemaFindings(before, after, ["public.captured"])[0].code,
    "CAPTURED_RELATION_CHANGED",
  );
  assert.equal(
    schemaFindings(before, after, [])[0].code,
    "INCOMPLETE_SCHEMA_OR_CAPTURE_INSPECTION",
  );
});
test("protected replacements, IAM and consumer activation are hard maintenance findings", () => {
  const before = {
    Resources: {
      Queue: { Type: "AWS::SQS::Queue", Properties: { VisibilityTimeout: 60 } },
      Map: {
        Type: "AWS::Lambda::EventSourceMapping",
        Properties: { Enabled: false, FunctionName: "old" },
      },
    },
  };
  const after = structuredClone(before);
  after.Resources.Queue.Properties.VisibilityTimeout = 90;
  after.Resources.Map.Properties.Enabled = true;
  after.Resources.Role = { Type: "AWS::IAM::Role", Properties: {} };
  assert.deepEqual(
    resourceFindings(before, after, [
      {
        ResourceChange: {
          ResourceType: "AWS::SQS::Queue",
          Action: "Modify",
          Replacement: "Conditional",
          LogicalResourceId: "Queue",
        },
      },
    ]).map((item) => item.code),
    [
      "PROTECTED_RESOURCE_CHANGED",
      "CONSUMER_TOPOLOGY_OR_ACTIVATION_CHANGED",
      "IAM_CHANGE_REQUIRES_MAINTENANCE",
      "PROTECTED_OR_ACTIVATION_CHANGE_SET_EFFECT",
      "PROTECTED_REPLACEMENT",
    ],
  );
});
test("every approval identity is revalidated and native no-change is explicit", async () => {
  const approved = Object.fromEntries(
    [
      "baselineDigest",
      "ledgerDigest",
      "configDigest",
      "artifactDigest",
      "changeSetDigest",
      "prerequisiteDigest",
      "inventoryDigest",
    ].map((key) => [key, "a"]),
  );
  assert.equal(revalidate(approved, approved), true);
  assert.throws(
    () => revalidate(approved, { ...approved, ledgerDigest: "b" }),
    /Stale ledgerDigest/,
  );
  const ctx = { target: "prod" },
    input = {
      stackName: "application-prod-compute",
      templateUrl: "https://bucket/template",
      parameters: [],
      roleArn: "arn:role",
      templateDigest: digest({ Resources: {} }),
      requestId: "a".repeat(64),
    };
  const value = await prepareChangeSet(ctx, input, {
    api: (_ctx, _service, operation) =>
      operation === "create-change-set"
        ? { Id: "changeset", StackId: "stack" }
        : {
            ChangeSetId: "changeset",
            StackId: "stack",
            Status: "FAILED",
            StatusReason: "The submitted information didn't contain changes.",
            Parameters: [],
          },
  });
  assert.equal(value.status, "NO_CHANGE");
});
test("real isolated PostgreSQL snapshot detects additive captured DDL", async () => {
  const migrations = [
    {
      version: 1,
      path: "migrations/1.sql",
      sql: "CREATE TABLE captured(id bigint PRIMARY KEY);",
    },
    {
      version: 2,
      path: "migrations/2.sql",
      sql: "ALTER TABLE captured ADD COLUMN private_email text;",
    },
  ].map((item) => ({
    ...item,
    checksum: createHash("sha384").update(item.sql).digest("hex"),
  }));
  const result = await snapshots(migrations, [
    { version: 1, checksum: migrations[0].checksum, success: true },
  ]);
  assert.equal(result.before[0].columns.length, 1);
  assert.equal(result.after[0].columns.length, 2);
  assert.equal(
    schemaFindings(result.before, result.after, ["public.captured"])[0].code,
    "CAPTURED_RELATION_CHANGED",
  );
  assert.notEqual(digest(result.before), digest(result.after));
});

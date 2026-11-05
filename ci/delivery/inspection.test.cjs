"use strict";
const { test } = require("node:test");
const assert = require("node:assert/strict");
const { createHash } = require("node:crypto");
const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");
const { execFileSync } = require("node:child_process");
const {
  cumulativeSource,
  ledgerFindings,
  schemaFindings,
  resourceFindings,
  revalidate,
  prepareChangeSet,
} = require("./inspection.cjs");
const { snapshots } = require("./schema.cjs");
const { digest } = require("./contracts.cjs");

test("source inventory retains full file versions under its separate bounded budget", (t) => {
  const repo = fs.mkdtempSync(path.join(os.tmpdir(), "aura-source-inventory-"));
  t.after(() => fs.rmSync(repo, { recursive: true, force: true }));
  const git = (...args) => execFileSync("git", ["-C", repo,
    "-c", "user.name=Delivery test", "-c", "user.email=delivery@example.invalid",
    "-c", "commit.gpgsign=false", "-c", "core.hooksPath=/dev/null", ...args],
    { encoding: "utf8", stdio: ["ignore", "pipe", "pipe"] }).trim();
  git("init", "--quiet");
  fs.mkdirSync(path.join(repo, "src"));
  const before = "unchanged context\n".repeat(250000), after = before + "candidate change\n";
  fs.writeFileSync(path.join(repo, "src/example.rs"), before);
  git("add", ".");
  git("commit", "--quiet", "-m", "Baseline");
  const baseline = git("rev-parse", "HEAD");
  fs.writeFileSync(path.join(repo, "src/example.rs"), after);
  git("commit", "--quiet", "-am", "Candidate");
  const candidate = git("rev-parse", "HEAD");
  const source = cumulativeSource(repo, baseline, candidate);
  assert.ok(Buffer.byteLength(JSON.stringify(source)) > 8 * 1024 * 1024);
  assert.deepEqual(source.context.find((item) => item.path === "src/example.rs"),
    { path: "src/example.rs", before, after });
  assert.match(source.diff, /\+candidate change/);
  assert.throws(() => cumulativeSource(repo, baseline, candidate, 1500000),
    /source inventory exceeds bounded budget/);
  fs.writeFileSync(path.join(repo, "src/extra.rs"), "additional context\n".repeat(200000));
  git("add", ".");
  git("commit", "--quiet", "-m", "Oversized candidate");
  assert.throws(() => cumulativeSource(repo, baseline, git("rev-parse", "HEAD")),
    /source inventory exceeds bounded budget/);
});

test("source inventory omits non-release data and binary patch bodies", (t) => {
  const repo = fs.mkdtempSync(path.join(os.tmpdir(), "aura-source-inventory-"));
  t.after(() => fs.rmSync(repo, { recursive: true, force: true }));
  const git = (...args) => execFileSync("git", ["-C", repo,
    "-c", "user.name=Delivery test", "-c", "user.email=delivery@example.invalid",
    "-c", "commit.gpgsign=false", "-c", "core.hooksPath=/dev/null", ...args],
    { encoding: "utf8", stdio: ["ignore", "pipe", "pipe"] }).trim();
  git("init", "--quiet");
  fs.mkdirSync(path.join(repo, "src"));
  fs.writeFileSync(path.join(repo, "src/example.rs"), "baseline\n");
  git("add", ".");
  git("commit", "--quiet", "-m", "Baseline");
  const baseline = git("rev-parse", "HEAD");
  fs.mkdirSync(path.join(repo, "data"));
  fs.writeFileSync(path.join(repo, "data/snapshot.jsonl"), "{\"row\":1}\n".repeat(2000000));
  fs.writeFileSync(path.join(repo, "src/asset.bin"), Buffer.alloc(4 * 1024 * 1024, 0));
  fs.writeFileSync(path.join(repo, "src/example.rs"), "candidate\n");
  git("add", ".");
  git("commit", "--quiet", "-m", "Candidate");
  const source = cumulativeSource(repo, baseline, git("rev-parse", "HEAD"), 1024 * 1024);
  assert.doesNotMatch(source.diff, /data\/snapshot/);
  assert.match(source.diff, /^Binary files \/dev\/null and b\/src\/asset\.bin differ$/m);
  assert.match(source.diff, /\+candidate/);
  assert.deepEqual(source.context.map((item) => item.path).sort(),
    ["docs/durable-worker-runbook.md", "docs/events/flow.md", "src/example.rs"]);
  fs.writeFileSync(path.join(repo, "src/example.rs"), "oversized\n".repeat(200000));
  git("commit", "--quiet", "-am", "Oversized candidate");
  assert.throws(() => cumulativeSource(repo, baseline, git("rev-parse", "HEAD"), 1024 * 1024),
    /source inventory exceeds bounded budget/);
});

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
    api: (_ctx, _service, operation, args) => {
      if (operation === "create-change-set")
        return { Id: "changeset", StackId: "stack" };
      assert.ok(args.includes("--include-property-values"));
      return {
        ChangeSetId: "changeset",
        StackId: "stack",
        Status: "FAILED",
        StatusReason: "The submitted information didn't contain changes.",
        Parameters: [],
      };
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

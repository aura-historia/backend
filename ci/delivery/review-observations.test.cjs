"use strict";
const { test } = require("node:test");
const assert = require("node:assert/strict");
const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");
const { execFileSync } = require("node:child_process");
const { collect, nativeObservations, templateComparison, revalidateRules } = require("./review-observations.cjs");
const { digest } = require("./contracts.cjs");
const fixture = require("./fixtures.cjs");
const ctx = { target: "prod", account: "123456789012", region: "eu-central-1", bucket: "release-bucket" };
const arn = `arn:aws:lambda:${ctx.region}:${ctx.account}:function:api-prod`;
const rule = { eventBusName: "provider-prod", name: "retired-forwarder" };
const now = new Date("2026-10-26T12:00:00Z");
const live = {
  activation: [{ bus: "provider-prod", targets: [{ arn: "production-queue" }] }],
  functions: [{ arn }],
  stacks: { data: { resources: [{ type: "AWS::RDS::DBInstance", physicalId: "database-prod" }] } },
};
function absent() {
  const error = new Error("AWS operation failed", { cause: { stderr: "An error occurred (ResourceNotFoundException)" } });
  throw error;
}

test("mail prefix rotation exposes identical content, while changed template bytes remain visible", () => {
  const baseline = { build: { id: "before" }, artifacts: [{ id: "email.html", kind: "TEMPLATE", sha256: fixture.hash, size: 42 }] };
  const candidate = structuredClone(baseline); candidate.build.id = "after";
  assert.equal(templateComparison(baseline, candidate).identicalContent, true);
  assert.equal(templateComparison(baseline, candidate).candidatePrefix, "mail/after/");
  candidate.artifacts[0].sha256 = "c".repeat(64);
  const comparison = templateComparison(baseline, candidate);
  assert.equal(comparison.identicalContent, false);
  assert.equal(comparison.baseline[0].sha256, fixture.hash);
  assert.equal(comparison.candidate[0].sha256, "c".repeat(64));
});

test("capacity evidence retains actual samples and only allowlisted runtime configuration", () => {
  const api = (_ctx, service, operation) => {
    if (service === "rds") return { DBInstances: [{ DBInstanceIdentifier: "database-prod", DBInstanceClass: "db.t4g.medium", DBInstanceStatus: "available", AllocatedStorage: 50 }] };
    if (service === "lambda") return { FunctionArn: arn, RevisionId: "revision", Environment: { Variables: {
      POSTGRES_MAX_CONNECTIONS: "1", PRODUCT_LISTING_SEARCH_PARALLEL_ENRICHMENT_ENABLED: "false", POSTGRES_PASSWORD: "never-in-evidence",
    } } };
    assert.equal(operation, "get-metric-statistics");
    return { Datapoints: [{ Timestamp: "2026-10-26T11:55:00Z", Maximum: 9, Minimum: 1000, Unit: "Count" }] };
  };
  const result = nativeObservations(ctx, { databaseInstanceId: "database-prod", lambdaFunctions: ["api-prod"] }, live, fixture.sha, ".", api, now);
  assert.equal(result.database.metrics[0].points[0].value, 9);
  assert.equal(result.database.metrics[2].statistic, "Minimum");
  assert.equal(result.functions[0].configuration.POSTGRES_MAX_CONNECTIONS, "1");
  assert.doesNotMatch(JSON.stringify(result), /never-in-evidence|POSTGRES_PASSWORD/);
  const empty = nativeObservations(ctx, { lambdaFunctions: ["api-prod"] }, live, fixture.sha, ".",
    (...args) => args[1] === "lambda" ? api(...args) : { Datapoints: [] }, now);
  assert.deepEqual(empty.functions[0].concurrency.points, []);
});

test("native checks reject resources outside the observed target and malformed metrics", () => {
  const api = () => { throw new Error("Unexpected cloud request"); };
  for (const checks of [{ databaseInstanceId: "other-db" }, { lambdaFunctions: ["other-prod"] },
    { retiredEventRules: [{ ...rule, eventBusName: "other-bus" }] }, { sourcePaths: ["src/../secret.rs"] },
    { sourcePaths: ["/private/secret.rs"] }, { unknown: true }, { lambdaFunctions: "api-prod" }])
    assert.throws(() => nativeObservations(ctx, checks, live, fixture.sha, ".", api, now));
  assert.throws(() => nativeObservations(ctx, { lambdaFunctions: ["api-prod"] }, live, fixture.sha, ".",
    (_ctx, service) => service === "lambda" ? { FunctionArn: arn, RevisionId: "revision" } : { Datapoints: [{ Timestamp: "invalid", Maximum: 1 }] }, now), /Invalid capacity/);
});

test("retirement evidence requires actual absence and is checked again before execution", () => {
  const result = nativeObservations(ctx, { retiredEventRules: [rule] }, live, fixture.sha, ".", absent, now);
  assert.equal(result.retiredEventRules[0].observedState, "ABSENT");
  revalidateRules(ctx, result, absent);
  assert.throws(() => revalidateRules(ctx, result, () => ({ State: "ENABLED" })), /still exists/);
  assert.throws(() => nativeObservations(ctx, { retiredEventRules: [rule] }, live, fixture.sha, ".",
    () => { throw new Error("AccessDenied"); }, now), /AccessDenied/);
});

test("supporting source is read from the candidate commit rather than the working tree", () => {
  const repo = fs.mkdtempSync(path.join(os.tmpdir(), "aura-review-source-"));
  const git = (...args) => execFileSync("git", ["-C", repo, "-c", "user.name=Test", "-c", "user.email=test@example.invalid", ...args], { encoding: "utf8", stdio: ["ignore", "pipe", "pipe"] }).trim();
  try {
    fs.mkdirSync(path.join(repo, "src/reader"), { recursive: true });
    const file = "src/reader/lib.rs"; fs.writeFileSync(path.join(repo, file), "candidate source");
    git("init", "--quiet"); git("add", "."); git("commit", "--quiet", "-m", "candidate");
    const sha = git("rev-parse", "HEAD"); fs.writeFileSync(path.join(repo, file), "uncommitted replacement");
    const result = nativeObservations(ctx, { sourcePaths: [file] }, live, sha, repo, () => {}, now);
    assert.equal(result.supportingSources[0].content, "candidate source");
    assert.equal(result.supportingSources[0].sourceSha, sha);
  } finally { fs.rmSync(repo, { recursive: true, force: true }); }
});

test("template evidence rejects a baseline manifest that disagrees with the verified release", () => {
  const bundle = fixture.bundle();
  const baseline = { lastVerified: { bundleDigest: digest(bundle), sourceSha: bundle.sourceSha } };
  const request = { baseline, bundle, live, repo: "." };
  assert.equal(collect(ctx, request, { store: { readJson: () => bundle }, api: () => {}, now }).templateArtifacts.identicalContent, true);
  const tampered = structuredClone(bundle); tampered.build.id = "a".repeat(64);
  assert.throws(() => collect(ctx, request, { store: { readJson: () => tampered }, api: () => {}, now }));
});

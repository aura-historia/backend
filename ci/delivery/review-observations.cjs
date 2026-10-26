"use strict";
const { execFileSync } = require("node:child_process");
const { bytesDigest, digest, validateBundle } = require("./contracts.cjs");

function templateComparison(baseline, candidate) {
  const templates = (bundle) => bundle.artifacts.filter((item) => item.kind === "TEMPLATE")
    .map(({ id, sha256, size }) => ({ id, sha256, size })).sort((a, b) => a.id.localeCompare(b.id));
  const before = templates(baseline), after = templates(candidate);
  return {
    baselinePrefix: `mail/${baseline.build.id}/`, candidatePrefix: `mail/${candidate.build.id}/`,
    baseline: before, candidate: after, identicalContent: digest(before) === digest(after),
  };
}

function ruleAbsent(ctx, rule, api) {
  try {
    api(ctx, "events", "describe-rule", ["--event-bus-name", rule.eventBusName, "--name", rule.name]);
  } catch (error) {
    if (error.code === "ResourceNotFoundException" ||
        /\(ResourceNotFoundException\)/.test(String(error.cause?.stderr || "")))
      return { ...rule, observedState: "ABSENT" };
    throw error;
  }
  throw new Error("Retired EventBridge rule still exists; fresh operator investigation required");
}

function revalidateRules(ctx, observations, api) {
  for (const rule of observations?.retiredEventRules || []) {
    const { name, eventBusName } = rule;
    ruleAbsent(ctx, { name, eventBusName }, api);
  }
}

function metric(ctx, namespace, name, dimensions, statistic, window, api) {
  const result = api(ctx, "cloudwatch", "get-metric-statistics", [
    "--namespace", namespace, "--metric-name", name,
    "--dimensions", JSON.stringify(Object.entries(dimensions).map(([Name, Value]) => ({ Name, Value }))),
    "--start-time", window.start, "--end-time", window.end, "--period", "300", "--statistics", statistic,
  ]);
  if (!Array.isArray(result.Datapoints)) throw new Error("Incomplete capacity metric observation");
  const points = result.Datapoints.map((point) => {
    if (!Number.isFinite(point[statistic]) || !Number.isFinite(Date.parse(point.Timestamp)))
      throw new Error("Invalid capacity metric datapoint");
    return { timestamp: point.Timestamp, value: point[statistic], unit: point.Unit };
  }).filter((point) => Date.parse(point.timestamp) >= Date.parse(window.start) && Date.parse(point.timestamp) <= Date.parse(window.end))
    .sort((a, b) => Date.parse(a.timestamp) - Date.parse(b.timestamp));
  return { namespace, name, dimensions, statistic, window, points };
}

function supportingSources(repo, sourceSha, paths) {
  return paths.map((file) => {
    if (typeof file !== "string" || !/^src\/[A-Za-z0-9_./-]+\.rs$/.test(file) ||
        file.split("/").some((part) => [".", ".."].includes(part)))
      throw new Error("Supporting review source must be a tracked Rust source path");
    const content = execFileSync("git", ["-C", repo, "show", `${sourceSha}:${file}`],
      { encoding: "utf8", maxBuffer: 500000, stdio: ["ignore", "pipe", "pipe"] });
    return { path: file, sourceSha, sha256: bytesDigest(Buffer.from(content)), content };
  });
}

function nativeObservations(ctx, checks, live, sourceSha, repo, api, now) {
  const allowed = ["retiredEventRules", "databaseInstanceId", "lambdaFunctions", "sourcePaths"];
  if (!checks || typeof checks !== "object" || Array.isArray(checks) ||
      Object.keys(checks).some((key) => !allowed.includes(key)))
    throw new Error("Unsupported native review check");
  for (const key of ["retiredEventRules", "lambdaFunctions", "sourcePaths"])
    if (checks[key] !== undefined && (!Array.isArray(checks[key]) || checks[key].length > 4))
      throw new Error("Native review check list exceeds its bounded shape");
  const window = { start: new Date(now.getTime() - 3600000).toISOString(), end: now.toISOString() };
  const buses = new Set(live.activation.filter((item) => item.bus).map((item) => item.bus));
  const retiredEventRules = (checks.retiredEventRules || []).map((rule) => {
    if (!rule || Object.keys(rule).sort().join() !== "eventBusName,name" || !buses.has(rule.eventBusName) ||
        typeof rule.name !== "string" || !/^[A-Za-z0-9_.-]{1,64}$/.test(rule.name))
      throw new Error("Retired rule must belong to an observed target event bus");
    return ruleAbsent(ctx, rule, api);
  });
  const resources = Object.values(live.stacks).flatMap((stack) => stack.resources);
  let database = null;
  if (checks.databaseInstanceId !== undefined) {
    if (!resources.some((item) => item.type === "AWS::RDS::DBInstance" && item.physicalId === checks.databaseInstanceId))
      throw new Error("Capacity observation must select the deployed target database");
    const result = api(ctx, "rds", "describe-db-instances", ["--db-instance-identifier", checks.databaseInstanceId]).DBInstances;
    if (result?.length !== 1 || result[0].DBInstanceIdentifier !== checks.databaseInstanceId)
      throw new Error("Incomplete target database identity");
    const value = result[0];
    if (typeof value.DBInstanceClass !== "string" || !value.DBInstanceClass.startsWith("db.") ||
        typeof value.DBInstanceStatus !== "string" || !Number.isInteger(value.AllocatedStorage) || value.AllocatedStorage <= 0)
      throw new Error("Incomplete target database capacity configuration");
    database = {
      identifier: value.DBInstanceIdentifier, instanceClass: value.DBInstanceClass,
      status: value.DBInstanceStatus, allocatedStorageGiB: value.AllocatedStorage,
      metrics: [["DatabaseConnections", "Maximum"], ["CPUUtilization", "Maximum"],
        ["FreeableMemory", "Minimum"], ["DiskQueueDepth", "Maximum"]].map(([name, statistic]) =>
        metric(ctx, "AWS/RDS", name, { DBInstanceIdentifier: value.DBInstanceIdentifier }, statistic, window, api)),
    };
  }
  const functions = (checks.lambdaFunctions || []).map((name) => {
    const arn = `arn:aws:lambda:${ctx.region}:${ctx.account}:function:${name}`;
    if (!live.functions.some((item) => item.arn === arn))
      throw new Error("Runtime observation must select an observed target function");
    const value = api(ctx, "lambda", "get-function-configuration", ["--function-name", name]);
    if (value.FunctionArn !== arn || typeof value.RevisionId !== "string" || !value.RevisionId)
      throw new Error("Observed function identity changed or is incomplete");
    const environment = value.Environment?.Variables || {};
    if ((environment.POSTGRES_MAX_CONNECTIONS !== undefined && !/^[1-9][0-9]*$/.test(environment.POSTGRES_MAX_CONNECTIONS)) ||
        (environment.PRODUCT_LISTING_SEARCH_PARALLEL_ENRICHMENT_ENABLED !== undefined &&
          !["true", "false"].includes(environment.PRODUCT_LISTING_SEARCH_PARALLEL_ENRICHMENT_ENABLED)))
      throw new Error("Invalid allowlisted runtime configuration");
    return {
      arn, revision: value.RevisionId,
      configuration: Object.fromEntries(["POSTGRES_MAX_CONNECTIONS", "PRODUCT_LISTING_SEARCH_PARALLEL_ENRICHMENT_ENABLED"]
        .filter((key) => Object.hasOwn(environment, key)).map((key) => [key, environment[key]])),
      concurrency: metric(ctx, "AWS/Lambda", "ConcurrentExecutions", { FunctionName: name }, "Maximum", window, api),
    };
  });
  return { observedAt: now.toISOString(), retiredEventRules, database, functions,
    supportingSources: supportingSources(repo, sourceSha, checks.sourcePaths || []) };
}

function collect(ctx, { baseline, bundle, live, checks, repo }, { api, store, now = new Date() }) {
  const previous = store.readJson(ctx.bucket, `bundles/${baseline.lastVerified.bundleDigest}.json`);
  validateBundle(previous);
  if (digest(previous) !== baseline.lastVerified.bundleDigest || previous.sourceSha !== baseline.lastVerified.sourceSha)
    throw new Error("Baseline artifact manifest differs from the verified release");
  return { templateArtifacts: templateComparison(previous, bundle),
    native: nativeObservations(ctx, checks || {}, live, bundle.sourceSha, repo, api, now) };
}

module.exports = { collect, nativeObservations, templateComparison, revalidateRules };

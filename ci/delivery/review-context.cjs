"use strict";
const { digest } = require("./contracts.cjs");

function objectChanges(before, after, path = "") {
  if (digest(before ?? null) === digest(after ?? null)) return [];
  if (
    before && after && typeof before === "object" && typeof after === "object" &&
    !Array.isArray(before) && !Array.isArray(after)
  )
    return [...new Set([...Object.keys(before), ...Object.keys(after)])]
      .sort()
      .flatMap((key) => {
        const next = `${path}/${key.replace(/~/g, "~0").replace(/\//g, "~1")}`;
        if (!Object.hasOwn(before, key))
          return [{ path: next, change: "ADD", after: after[key] }];
        if (!Object.hasOwn(after, key))
          return [{ path: next, change: "REMOVE", before: before[key] }];
        return objectChanges(before[key], after[key], next);
      });
  return [{ path, change: "REPLACE", before, after }];
}

function relations(schema) {
  if (!Array.isArray(schema)) throw new Error("Complete schema snapshot required");
  const values = {};
  for (const row of schema) {
    if (typeof row.relation !== "string" || !row.relation || Object.hasOwn(values, row.relation))
      throw new Error("Unique schema relation identities required");
    Object.defineProperty(values, row.relation, { value: row, enumerable: true });
  }
  return values;
}

function nativeEffects(changes) {
  return changes.map(({ ResourceChange: resource }) => ({
    action: resource.Action,
    logicalId: resource.LogicalResourceId,
    type: resource.ResourceType,
    replacement: resource.Replacement || null,
    properties: (resource.Details || []).map((detail) => detail.Target),
  }));
}

function infrastructureContext(inventory) {
  const byStack = new Map(inventory.plans.map((plan) => [plan.stackName, plan]));
  return Object.entries(inventory.aws.stacks).map(([name, stack]) => {
    const plan = byStack.get(name), comparison = inventory.templateComparisons[name];
    if (!comparison) throw new Error("Every stack requires a template comparison");
    return {
      stack: name,
      status: stack.status,
      protectedPlatform: !plan,
      ...comparison,
      changes: plan ? objectChanges(stack.template, plan.template) : [],
      nativePlan: plan ? {
        id: plan.changeSet.id,
        status: plan.changeSet.status,
        effects: nativeEffects(plan.changeSet.changes),
      } : null,
    };
  });
}

function databaseContext(inventory) {
  const { source, database, schema } = inventory;
  const applied = new Set(database.ledger.map((row) => row.version));
  return {
    ledger: database.ledger,
    ledgerFingerprint: database.ledger_fingerprint,
    actualSchemaDigest: digest(database.schema),
    baselineSchemaDigest: digest(schema.before),
    candidateSchemaDigest: digest(schema.after),
    actualMatchesMigrationHistory: digest(database.schema) === digest(schema.before),
    schemaChanges: objectChanges(relations(schema.before), relations(schema.after)),
    pendingMigrations: source.migrations.filter((migration) => !applied.has(migration.version)),
    capturedRelations: inventory.capturedRelations,
  };
}

function consumerContext(aws) {
  return {
    activationDigest: digest(aws.activation),
    bindingCount: aws.activation.length,
    states: aws.activation.reduce((counts, item) => {
      counts[item.state] = (counts[item.state] || 0) + 1;
      return counts;
    }, {}),
    functionInventoryDigest: digest(aws.functions),
    functionCount: aws.functions.length,
  };
}

function reviewContext(inventory) {
  const { source, database, schema, aws, plans, config, baselineState, templateComparisons } = inventory;
  if (
    !source || !database || !schema || !aws?.stacks || !Array.isArray(plans) ||
    !config || !baselineState?.lastVerified || !templateComparisons ||
    !Array.isArray(source.migrations) || typeof source.diff !== "string"
  )
    throw new Error("Complete verified inventory required for focused review");
  const sourceDiff = source.diff.split(/(?=^diff --git )/m)
    .filter((patch) => !/^diff --git a\/migrations\//.test(patch)).join("");
  return {
    version: "compatibility-context-v1",
    inventoryDigest: digest(inventory),
    release: {
      target: config.target, account: config.account, region: config.region,
      observedAt: inventory.observedAt,
      baseline: baselineState,
      candidateBundleDigest: inventory.candidateBundleDigest,
      source: { baseline: source.baseline, candidate: source.candidate },
    },
    source: {
      mergedCommitNotes: source.commits,
      changedSourceAndContractDiff: sourceDiff,
    },
    database: databaseContext(inventory),
    infrastructure: infrastructureContext(inventory),
    capture: { ...aws.capture, ...inventory.dmsBaseline, health: aws.health },
    consumers: consumerContext(aws),
    search: inventory.search,
    patches: inventory.patches,
    manualPrerequisites: inventory.manualPrerequisites || [],
    fullSuite: inventory.tests,
    hardFindings: inventory.hardFindings,
  };
}

function serializeContext(evidence, config) {
  const serialized = JSON.stringify(evidence);
  if (Buffer.byteLength(serialized) > (config.maxInputBytes || 1500000))
    throw new Error(
      "Complete context exceeds reviewer budget; explicit human review required",
    );
  return serialized;
}

module.exports = { reviewContext, serializeContext };

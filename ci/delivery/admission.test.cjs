"use strict";
const { test } = require("node:test");
const assert = require("node:assert/strict");
const { digest } = require("./contracts.cjs");
const admission = require("./admission.cjs");
const maintenance = require("./maintenance.cjs");
const { hash, sha, time } = require("./fixtures.cjs");
function memory() {
  let state = {
    version: 1,
    target: "prod",
    revision: 0,
    state: "READY",
    lastVerified: { bundleDigest: hash, sourceSha: sha, verifiedAt: time },
    observedSchema: { sha256: hash, observedAt: time, migrations: [] },
    active: null,
    holdReason: null,
  };
  return {
    read: () => ({
      state: structuredClone(state),
      etag: String(state.revision),
    }),
    write: (next, etag) => {
      assert.equal(etag, String(state.revision));
      state = next;
    },
  };
}
const execution = {
  executionId: "native-A",
  requestDigest: hash,
  bundleDigest: hash,
};
test("native admission excludes other releases and maintenance, failures retain hold and verified app baseline", () => {
  const store = memory(),
    baseline = store.read().state;
  const active = admission.acquire(store, baseline, execution);
  assert.equal(
    admission.acquire(store, baseline, execution).revision,
    active.revision,
  );
  assert.throws(
    () =>
      admission.acquire(store, baseline, {
        ...execution,
        executionId: "native-B",
      }),
    /baseline changed/,
  );
  assert.throws(
    () =>
      admission.enterMaintenance(
        store,
        active,
        { ...execution, executionId: "maintenance" },
        "DDL",
      ),
    /active operation/,
  );
  admission.fail(store, execution, "migration committed; application failed");
  const held = store.read().state;
  assert.equal(held.state, "NEEDS_ATTENTION");
  assert.deepEqual(held.lastVerified, baseline.lastVerified);
  assert.throws(
    () =>
      admission.reconcile(store, held, {
        operator: "human",
        reason: "runner lost",
        nativeWorkSettled: false,
      }),
    /settled evidence/,
  );
  admission.reconcile(store, held, {
    operator: "human",
    reason: "native work settled; forward fix required",
    nativeWorkSettled: true,
    observedSchema: { ...held.observedSchema, sha256: "c".repeat(64) },
    evidenceDigest: hash,
  });
  assert.equal(store.read().state.state, "READY");
  assert.deepEqual(store.read().state.lastVerified, baseline.lastVerified);
});
function rawTask(status = "running") {
  return {
    ReplicationTaskArn: "arn:task",
    SourceEndpointArn: "arn:source",
    TargetEndpointArn: "arn:target",
    ReplicationInstanceArn: "arn:instance",
    MigrationType: "cdc",
    TableMappings: "{}",
    ReplicationTaskSettings: "{}",
    RecoveryCheckpoint: "checkpoint:V1#same",
    Status: status,
    StopReason: "Stop Reason NORMAL",
    ReplicationTaskStats: { StartDate: time, TablesErrored: 0 },
  };
}
function describeTask(task, operation, args) {
  if (operation === "describe-replication-tasks")
    return { ReplicationTasks: [task] };
  if (operation === "describe-endpoints")
    return {
      Endpoints: [{
        EndpointArn: args[1].split("Values=")[1],
        EngineName: "postgres",
        PostgreSQLSettings: { SlotName: "owned-slot" },
      }],
    };
  throw new Error(operation);
}
const wal = () => ({
  maxPauseSeconds: 600,
  retainedBytes: 1,
  freeBytes: 100,
  monitoringUri: "restricted://wal",
  observedAt: new Date().toISOString(),
});
test("ambiguous DMS stop/resume is observed once and deliberate stopped state stays stopped", async () => {
  const store = memory();
  admission.enterMaintenance(
    store,
    store.read().state,
    execution,
    "reviewed DDL",
  );
  let task = rawTask(),
    stop = 0,
    start = 0;
  const journal = [];
  const api = (_ctx, _service, operation, args) => {
    if (operation === "stop-replication-task") {
      stop++;
      task = rawTask("stopped");
      throw new Error("lost response");
    }
    if (operation === "start-replication-task") {
      start++;
      task = rawTask("running");
      throw new Error("lost response");
    }
    return describeTask(task, operation, args);
  };
  const persist = async (kind, value) =>
    journal.push({ kind, digest: digest(value) });
  const pause = await maintenance.pause(
    {},
    store,
    execution,
    "arn:task",
    wal(),
    persist,
    { api, delay: async () => {} },
  );
  await maintenance.resume({}, store, execution, pause, persist, {
    api,
    delay: async () => {},
  });
  assert.equal(stop, 1);
  assert.equal(start, 1);
  assert.equal(journal[0].kind, "pause-intent");
  assert.equal(store.read().state.state, "MAINTENANCE");
  task = rawTask("stopped");
  const stopped = await maintenance.pause(
    {},
    store,
    execution,
    "arn:task",
    wal(),
    persist,
    { api },
  );
  assert.equal(
    (await maintenance.resume({}, store, execution, stopped, persist, { api }))
      .status,
    "PRESERVED_STOPPED",
  );
  assert.equal(start, 1);
  assert.throws(
    () => maintenance.walBudget({ ...wal(), observedAt: "invalid" }),
    /Fresh WAL/,
  );
});
test("stopped pipeline with abandoned action cannot hide a running CodeBuild mutation", () => {
  const api = (_ctx, service, operation) => {
    if (operation === "get-pipeline-execution")
      return {
        pipelineExecution: {
          pipelineExecutionId: execution.executionId,
          status: "Stopped",
        },
      };
    if (operation === "list-action-executions")
      return {
        actionExecutionDetails: [
          {
            actionExecutionId: "action",
            actionName: "Migrate",
            status: "Abandoned",
            input: { actionTypeId: { provider: "CodeBuild" } },
            output: { executionResult: { externalExecutionId: "build:1" } },
          },
        ],
      };
    if (operation === "batch-get-builds")
      return { builds: [{ id: "build:1", buildStatus: "IN_PROGRESS" }] };
    if (operation === "describe-stacks")
      return { Stacks: [{ StackId: "stack", StackStatus: "UPDATE_COMPLETE" }] };
    if (operation === "list-stack-resources")
      return { StackResourceSummaries: [] };
    throw new Error(`${service}/${operation}`);
  };
  const observed = maintenance.inspectNative(
    { pipelineName: "pipeline" },
    execution,
    ["stack"],
    [],
    api,
  );
  assert.equal(observed.nativeWorkSettled, false);
  assert.equal(observed.builds[0].status, "IN_PROGRESS");
});

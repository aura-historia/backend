"use strict";
const { digest } = require("./contracts.cjs");
const { aws, enterMaintenance, reconcile } = require("./admission.cjs");
function readTask(ctx, arn, api = aws) {
  const tasks = api(ctx, "dms", "describe-replication-tasks", [
    "--filters",
    `Name=replication-task-arn,Values=${arn}`,
  ]).ReplicationTasks;
  const task = tasks?.length === 1 && tasks[0];
  if (
    !task ||
    task.ReplicationTaskArn !== arn ||
    task.MigrationType !== "cdc" ||
    !/^checkpoint:V1#\S+$/.test(task.RecoveryCheckpoint || "") ||
    !Number.isFinite(Date.parse(task.ReplicationTaskStats?.StartDate))
  )
    throw new Error(
      "Missing/replaced/unstarted CDC task or checkpoint; use specialized recovery",
    );
  const endpoints = [task.SourceEndpointArn, task.TargetEndpointArn].map(
    (endpointArn) => {
      const result = api(ctx, "dms", "describe-endpoints", [
        "--filters",
        `Name=endpoint-arn,Values=${endpointArn}`,
      ]).Endpoints;
      if (result?.length !== 1 || result[0].EndpointArn !== endpointArn)
        throw new Error("Missing maintenance endpoint/slot identity");
      const value = result[0];
      return {
        arn: endpointArn,
        engine: value.EngineName,
        database: value.DatabaseName || null,
        slot: value.PostgreSQLSettings?.SlotName || null,
        plugin: value.PostgreSQLSettings?.PluginName || null,
        stream: value.KinesisSettings?.StreamArn || null,
        extraDigest: digest(value.ExtraConnectionAttributes || ""),
      };
    },
  );
  const captureDigest = digest({
    mappings: task.TableMappings,
    settings: task.ReplicationTaskSettings,
    endpoints,
  });
  return {
    arn,
    captureDigest,
    observedAt: new Date().toISOString(),
    settingsHash: require("./health.cjs").fingerprint(
      task.ReplicationTaskSettings,
    ),
    mappingsHash: require("./health.cjs").fingerprint(task.TableMappings),
    source: task.SourceEndpointArn,
    target: task.TargetEndpointArn,
    instance: task.ReplicationInstanceArn,
    checkpoint: task.RecoveryCheckpoint,
    status: task.Status,
    stopReason: task.StopReason || null,
    tablesErrored: task.ReplicationTaskStats.TablesErrored,
  };
}
function sameTask(actual, recorded) {
  for (const key of ["arn", "source", "target", "instance", "captureDigest"])
    if (!actual[key] || actual[key] !== recorded[key])
      throw new Error(`DMS ${key} changed; maintenance hold retained`);
}
function requireOwner(store, operation) {
  const state = store.read()?.state;
  if (
    state?.state !== "MAINTENANCE" ||
    !state.active ||
    digest(state.active) !== digest(operation)
  )
    throw new Error(
      "Explicit maintenance owner must hold the common environment boundary",
    );
  return state;
}
function walBudget(evidence, now = Date.now()) {
  if (
    !evidence ||
    !Number.isInteger(evidence.maxPauseSeconds) ||
    evidence.maxPauseSeconds < 60 ||
    evidence.maxPauseSeconds > 86400 ||
    !Number.isFinite(evidence.retainedBytes) ||
    !Number.isFinite(evidence.freeBytes) ||
    evidence.retainedBytes < 0 ||
    evidence.freeBytes <= evidence.retainedBytes ||
    !evidence.monitoringUri ||
    !Number.isFinite(Date.parse(evidence.observedAt)) ||
    now - Date.parse(evidence.observedAt) > 300000 ||
    Date.parse(evidence.observedAt) > now + 60000
  )
    throw new Error(
      "Fresh WAL/storage headroom and active monitoring evidence required before pause",
    );
}
async function wait(ctx, recorded, target, allowed, options) {
  const api = options.api || aws,
    delay =
      options.delay ||
      ((ms) => new Promise((resolve) => setTimeout(resolve, ms)));
  for (let attempt = 0; attempt < (options.attempts || 90); attempt++) {
    const task = readTask(ctx, recorded.arn, api);
    sameTask(task, recorded);
    if (task.tablesErrored !== 0)
      throw new Error("DMS table errors; hold retained for diagnosis");
    if (task.status === target) return task;
    if (!allowed.includes(task.status))
      throw new Error(`Unexpected DMS ${task.status}; hold retained`);
    await delay(10000);
  }
  throw new Error(
    `DMS ${target} outcome unknown after bounded observation; hold retained`,
  );
}
async function pause(
  ctx,
  store,
  operation,
  arn,
  evidence,
  persist,
  options = {},
) {
  requireOwner(store, operation);
  walBudget(evidence);
  if (typeof persist !== "function")
    throw new Error("Durable maintenance intent writer required");
  const api = options.api || aws,
    prior = readTask(ctx, arn, api);
  if (
    !["running", "stopped"].includes(prior.status) ||
    prior.tablesErrored !== 0 ||
    (prior.status === "stopped" &&
      !/^Stop Reason NORMAL$/i.test(prior.stopReason || ""))
  )
    throw new Error("DMS is not in a reviewed stable prior state");
  const intent = {
    version: 1,
    operation,
    prior,
    wal: evidence,
    pauseOwned: prior.status === "running",
    pauseIntentAt: new Date().toISOString(),
  };
  // The durable intent precedes any call that can complete ambiguously.
  await persist("pause-intent", intent);
  if (!intent.pauseOwned) {
    await persist("pause-observed", intent);
    return intent;
  }
  try {
    api(ctx, "dms", "stop-replication-task", ["--replication-task-arn", arn]);
  } catch {
    /* Observe native state; never blindly submit another stop. */
  }
  const stopped = await wait(
    ctx,
    prior,
    "stopped",
    ["running", "stopping"],
    options,
  );
  const completed = { ...intent, stopped, stoppedAt: new Date().toISOString() };
  await persist("pause-observed", completed);
  return completed;
}
async function resume(ctx, store, operation, recorded, persist, options = {}) {
  requireOwner(store, operation);
  if (
    digest(recorded.operation) !== digest(operation) ||
    !recorded.prior ||
    !recorded.pauseIntentAt ||
    !recorded.wal
  )
    throw new Error("Exact persisted maintenance pause identity required");
  if (!recorded.pauseOwned)
    return { status: "PRESERVED_STOPPED", prior: recorded.prior };
  if (
    Date.now() - Date.parse(recorded.pauseIntentAt) >
    recorded.wal.maxPauseSeconds * 1000
  )
    throw new Error(
      "WAL pause budget exceeded; inspect capacity and approve recovery while retaining hold",
    );
  const api = options.api || aws,
    task = readTask(ctx, recorded.prior.arn, api);
  sameTask(task, recorded.prior);
  if (!recorded.stopped || task.checkpoint !== recorded.stopped.checkpoint)
    throw new Error(
      "Stopped checkpoint changed or pause completion unknown; inspect before resume",
    );
  await persist("resume-intent", {
    operation,
    task,
    pauseDigest: digest(recorded),
    at: new Date().toISOString(),
  });
  if (task.status === "stopped") {
    try {
      api(ctx, "dms", "start-replication-task", [
        "--replication-task-arn",
        task.arn,
        "--start-replication-task-type",
        "resume-processing",
      ]);
    } catch {
      /* Reconcile the native task without inventing a start LSN. */
    }
  } else if (!["starting", "running"].includes(task.status))
    throw new Error("Unexpected DMS lifecycle before resume");
  const running = await wait(
    ctx,
    recorded.prior,
    "running",
    ["stopped", "starting"],
    options,
  );
  await persist("resume-observed", {
    operation,
    task: running,
    at: new Date().toISOString(),
  });
  return running;
}
function inspectNative(ctx, operation, stackNames, deploymentIds, api = aws) {
  const execution = api(ctx, "codepipeline", "get-pipeline-execution", [
    "--pipeline-name",
    ctx.pipelineName,
    "--pipeline-execution-id",
    operation.executionId,
  ]).pipelineExecution;
  if (!execution || execution.pipelineExecutionId !== operation.executionId)
    throw new Error("Unknown native pipeline execution");
  const actions = api(ctx, "codepipeline", "list-action-executions", [
    "--pipeline-name",
    ctx.pipelineName,
    "--filter",
    `pipelineExecutionId=${operation.executionId}`,
  ]).actionExecutionDetails;
  if (!Array.isArray(actions))
    throw new Error("Missing native action execution inventory");
  const builds = actions
    .filter((action) => action.input?.actionTypeId?.provider === "CodeBuild")
    .map((action) => {
      const id = action.output?.executionResult?.externalExecutionId;
      if (!id) throw new Error("Missing underlying CodeBuild execution ID");
      const result = api(ctx, "codebuild", "batch-get-builds", [
        "--ids",
        id,
      ]).builds;
      if (result?.length !== 1 || result[0].id !== id)
        throw new Error("Missing underlying CodeBuild state");
      return { id, status: result[0].buildStatus };
    });
  const { stacks, deployments } = inspectResources(
    ctx,
    stackNames,
    deploymentIds,
    api,
  );
  const settled =
    ["Succeeded", "Failed", "Stopped", "Superseded"].includes(
      execution.status,
    ) &&
    actions.every(
      (action) => !["InProgress", "NotStarted"].includes(action.status),
    ) &&
    stacks.every((stack) => !/_IN_PROGRESS$/.test(stack.status)) &&
    builds.every((build) =>
      ["SUCCEEDED", "FAILED", "FAULT", "STOPPED", "TIMED_OUT"].includes(
        build.status,
      ),
    ) &&
    deployments.every((deployment) =>
      ["Succeeded", "Failed", "Stopped"].includes(deployment.status),
    );
  return {
    executionId: operation.executionId,
    pipelineStatus: execution.status,
    actions: actions.map((action) => ({
      id: action.actionExecutionId,
      name: action.actionName,
      status: action.status,
      externalId: action.output?.executionResult?.externalExecutionId || null,
    })),
    stacks,
    builds,
    deployments,
    nativeWorkSettled: settled,
  };
}
function inspectResources(ctx, stackNames, deploymentIds, api) {
  const stacks = stackNames.map((name) => {
    const values = api(ctx, "cloudformation", "describe-stacks", [
      "--stack-name",
      name,
    ]).Stacks;
    if (values?.length !== 1) throw new Error("Missing native stack state");
    return { id: values[0].StackId, status: values[0].StackStatus };
  });
  const derived = new Set(deploymentIds);
  for (const name of stackNames) {
    const resources = api(ctx, "cloudformation", "list-stack-resources", [
      "--stack-name",
      name,
    ]).StackResourceSummaries;
    if (!Array.isArray(resources))
      throw new Error("Missing CodeDeploy ownership inventory");
    for (const resource of resources.filter(
      (item) => item.ResourceType === "AWS::CodeDeploy::Application",
    )) {
      const groups = api(ctx, "deploy", "list-deployment-groups", [
        "--application-name",
        resource.PhysicalResourceId,
      ]).deploymentGroups;
      if (!Array.isArray(groups))
        throw new Error("Missing CodeDeploy group inventory");
      for (const group of groups) {
        const ids = api(ctx, "deploy", "list-deployments", [
          "--application-name",
          resource.PhysicalResourceId,
          "--deployment-group-name",
          group,
          "--include-only-statuses",
          "Created",
          "Queued",
          "InProgress",
          "Baking",
          "Ready",
        ]).deployments;
        if (!Array.isArray(ids))
          throw new Error("Missing active CodeDeploy inventory");
        ids.forEach((id) => derived.add(id));
      }
    }
  }
  const deployments = [...derived].map((id) => {
    const value = api(ctx, "deploy", "get-deployment", [
      "--deployment-id",
      id,
    ]).deploymentInfo;
    if (!value || value.deploymentId !== id)
      throw new Error("Missing native CodeDeploy state");
    return {
      id,
      status: value.status,
      application: value.applicationName,
      group: value.deploymentGroupName,
    };
  });
  return { stacks, deployments };
}
function inspectManualNative(ctx, stackNames, api = aws) {
  const { stacks, deployments } = inspectResources(ctx, stackNames, [], api);
  const builds = [];
  for (const phase of ["admission", "migrate", "verify"]) {
    const ids = api(ctx, "codebuild", "list-builds-for-project", [
      "--project-name",
      `aura-delivery-${ctx.target}-${phase}`,
      "--sort-order",
      "DESCENDING",
    ]).ids;
    if (!Array.isArray(ids)) throw new Error("Missing provider work inventory");
    for (let index = 0; index < ids.length; index += 100) {
      const result = api(ctx, "codebuild", "batch-get-builds", [
        "--ids",
        ...ids.slice(index, index + 100),
      ]).builds;
      if (
        !Array.isArray(result) ||
        result.length !== ids.slice(index, index + 100).length
      )
        throw new Error("Incomplete provider work inventory");
      result.forEach((build) =>
        builds.push({ id: build.id, status: build.buildStatus }),
      );
    }
  }
  const nativeWorkSettled =
    stacks.every((stack) => !/_IN_PROGRESS$/.test(stack.status)) &&
    deployments.every((deployment) =>
      ["Succeeded", "Failed", "Stopped"].includes(deployment.status),
    ) &&
    builds.every((build) =>
      ["SUCCEEDED", "FAILED", "FAULT", "STOPPED", "TIMED_OUT"].includes(
        build.status,
      ),
    );
  return {
    executionId: ctx.executionId,
    stacks,
    deployments,
    builds,
    nativeWorkSettled,
  };
}
async function verifyMaintenanceCompletion(
  ctx,
  held,
  config,
  recordReader,
  api = aws,
) {
  const original = await recordReader("dms-baseline");
  if (!original || !held.active)
    throw new Error("Recorded pre-maintenance DMS baseline required");
  const actual = readTask(ctx, config.dmsTaskArn, api);
  sameTask(actual, original);
  if (
    actual.status !== original.status ||
    !["running", "stopped", "ready"].includes(actual.status) ||
    actual.tablesErrored !== 0
  )
    throw new Error(
      "Maintenance did not restore the intended DMS lifecycle/health",
    );
  const pause = await recordReader("pause-intent");
  if (pause?.pauseOwned) {
    const resumed = await recordReader("resume-observed");
    if (
      !resumed ||
      digest(resumed.operation) !== digest(held.active) ||
      resumed.task.status !== "running"
    )
      throw new Error("Owned DMS pause lacks recorded resume completion");
    sameTask(actual, resumed.task);
  }
  const { verifyCurrentHealth } = require("./execution.cjs");
  const health = await verifyCurrentHealth(
    ctx,
    config,
    {
      arn: original.arn,
      status: original.status,
      settingsHash: original.settingsHash,
      mappingsHash: original.mappingsHash,
    },
    original.observedAt,
    { api },
  );
  return { actual, health };
}
function reconcileObserved(store, expected, evidence, observations) {
  if (
    !evidence.migrationInvocationSettled ||
    !evidence.migrationEvidenceUri ||
    !observations.nativeWorkSettled ||
    observations.executionId !== expected.active?.executionId ||
    digest(observations) !== evidence.evidenceDigest
  )
    throw new Error("Exact settled native execution evidence required");
  return reconcile(store, expected, { ...evidence, nativeWorkSettled: true });
}
module.exports = {
  readTask,
  sameTask,
  walBudget,
  requireOwner,
  pause,
  resume,
  inspectNative,
  inspectManualNative,
  verifyMaintenanceCompletion,
  enterMaintenance,
  reconcileObserved,
};

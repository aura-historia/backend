'use strict';
const fs = require('node:fs');
const { digest } = require('./contracts.cjs');
const {
  trustedInvocation,
  approvedEnvironment,
  assertControlRevision,
} = require('./workflow.cjs');
const { loadTargetConfig, discover } = require('./preparation.cjs');
const { inspectDatabase } = require('./inspection.cjs');
const admission = require('./admission.cjs');
const maintenance = require('./maintenance.cjs');
const storage = require('./storage.cjs');
const { fingerprint } = require('./health.cjs');

function assertExpectedState(state, evidence) {
  if (!state || evidence.expectedStateDigest !== digest(state))
    throw new Error(
      'Expected state digest differs from actual environment; inspect and request fresh maintenance approval',
    );
  if (!evidence.reason || typeof evidence.reason !== 'string')
    throw new Error('Explicit reviewed maintenance reason required');
}
function observedSchema(result) {
  return {
    sha256: digest(result.schema),
    observedAt: new Date().toISOString(),
    migrations: result.ledger.map((row) => ({
      version: String(row.version),
      checksum: row.checksum,
    })),
  };
}
function readRecord(ctx, state, name) {
  const key = `maintenance/${ctx.target}/${digest(state.active)}/${name}.json`;
  const listed = storage.aws('s3api', 'list-objects-v2', [
    '--bucket',
    ctx.bucket,
    '--prefix',
    key,
  ]);
  return listed.Contents?.some((object) => object.Key === key)
    ? storage.readJson(ctx.bucket, key)
    : null;
}
async function continuePause(
  ctx,
  store,
  state,
  config,
  evidence,
  persist,
  read,
  options,
) {
  const intent = read('pause-intent'),
    observed = read('pause-observed');
  if (!intent)
    return maintenance.pause(
      ctx,
      store,
      state.active,
      config.dmsTaskArn,
      evidence.wal,
      persist,
      options,
    );
  maintenance.requireOwner(store, state.active);
  if (digest(intent.operation) !== digest(state.active))
    throw new Error('Persisted pause belongs to another owner');
  const task = maintenance.readTask(ctx, intent.prior.arn, options?.api);
  maintenance.sameTask(task, intent.prior);
  if (
    task.status !== 'stopped' ||
    task.tablesErrored !== 0 ||
    !/^Stop Reason NORMAL$/i.test(task.stopReason || '')
  )
    throw new Error(
      'Pause result remains unverified; inspect native DMS without issuing another stop',
    );
  if (observed) return observed;
  const completed = {
    ...intent,
    stopped: task,
    stoppedAt: new Date().toISOString(),
  };
  await persist('pause-observed', completed);
  return completed;
}
async function continueResume(ctx, store, state, persist, read, options) {
  const recorded = read('pause-observed');
  if (!recorded)
    throw new Error('Observed exact pause evidence required before resume');
  const intent = read('resume-intent'),
    observed = read('resume-observed');
  if (!intent && !observed)
    return maintenance.resume(
      ctx,
      store,
      state.active,
      recorded,
      persist,
      options,
    );
  maintenance.requireOwner(store, state.active);
  const task = maintenance.readTask(ctx, recorded.prior.arn, options?.api);
  maintenance.sameTask(task, recorded.prior);
  if (task.status !== 'running' || task.tablesErrored !== 0)
    throw new Error(
      'Resume result remains unverified; inspect native DMS without blindly resubmitting',
    );
  if (observed) return observed;
  const completed = {
    operation: state.active,
    task,
    at: new Date().toISOString(),
  };
  await persist('resume-observed', completed);
  return completed;
}
async function execute(action, ctx, config, evidence, deps = {}) {
  const store = deps.store || admission.stateStore(ctx);
  const persist =
    deps.persist ||
    ((name, value) =>
      storage.putImmutableJson(
        ctx.bucket,
        `maintenance/${ctx.target}/${digest(store.read().state.active)}/${name}.json`,
        value,
      ));
  try {
    const state = store.read()?.state;
    if (action === 'status')
      return { state, stateDigest: state ? digest(state) : null };
    if (action === 'adopt')
      return await require('./adoption.cjs').adopt(
        ctx,
        config,
        evidence,
        store,
        deps,
      );
    assertExpectedState(state, evidence);
    if (action === 'enter') {
      const operation = {
        executionId: `maintenance:${digest({ target: ctx.target, evidence, runId: process.env.GITHUB_RUN_ID || null, runAttempt: process.env.GITHUB_RUN_ATTEMPT || null })}`,
        requestDigest: digest(evidence),
        bundleDigest: evidence.bundleDigest || state.lastVerified?.bundleDigest,
      };
      const settled = (deps.inspectManual || maintenance.inspectManualNative)(
        { ...ctx, executionId: operation.executionId },
        config.stackNames,
      );
      if (!settled.nativeWorkSettled)
        throw new Error(
          'Native mutation work is still active; maintenance exclusion cannot be assumed',
        );
      const entered = maintenance.enterMaintenance(
        store,
        state,
        operation,
        evidence.reason,
      );
      await persist('entered', { operation, reason: evidence.reason });
      const task = (deps.readTask || maintenance.readTask)(
        ctx,
        config.dmsTaskArn,
      );
      const raw = (
        deps.readRawTask ||
        (() =>
          admission.aws(ctx, 'dms', 'describe-replication-tasks', [
            '--filters',
            `Name=replication-task-arn,Values=${config.dmsTaskArn}`,
          ]).ReplicationTasks?.[0])
      )();
      if (!raw)
        throw new Error(
          'Missing initial maintenance capture snapshot; hold retained',
        );
      await persist('dms-baseline', {
        ...task,
        observedAt: new Date().toISOString(),
        settingsHash: fingerprint(raw.ReplicationTaskSettings),
        mappingsHash: fingerprint(raw.TableMappings),
      });

      return { state: entered, stateDigest: digest(entered) };
    }
    if (!state.active)
      throw new Error(
        'An active durable maintenance/recovery owner is required',
      );
    const read = deps.readRecord || ((name) => readRecord(ctx, state, name));
    if (action === 'pause')
      return await continuePause(
        ctx,
        store,
        state,
        config,
        evidence,
        persist,
        read,
        deps.options,
      );
    if (action === 'resume')
      return await continueResume(
        ctx,
        store,
        state,
        persist,
        read,
        deps.options,
      );
    if (action === 'reconcile') {
      if (
        evidence.migrationInvocationSettled !== true ||
        !evidence.migrationEvidenceUri
      )
        throw new Error(
          'Explicit settled migration evidence required; elapsed time does not prove completion',
        );
      const observations = state.active.executionId.startsWith('maintenance:')
        ? (deps.inspectManual || maintenance.inspectManualNative)(
            { ...ctx, executionId: state.active.executionId },
            config.stackNames,
          )
        : (deps.inspectNative || maintenance.inspectNative)(
            {
              ...ctx,
              pipelineName: config.pipelineName || `aura-deploy-${ctx.target}`,
            },
            state.active,
            config.stackNames,
            [],
          );
      if (!observations.nativeWorkSettled)
        throw new Error(
          'Native work remains active or unverified; hold retained',
        );
      await (
        deps.verifyMaintenanceCompletion ||
        maintenance.verifyMaintenanceCompletion
      )(ctx, state, config, read);
      const inspection = (deps.inspectDatabase || inspectDatabase)(
        ctx,
        config.inspectorFunctionArn,
      );
      const proof = {
        operator: process.env.GITHUB_ACTOR || evidence.operator,
        reason: evidence.reason,
        migrationInvocationSettled: true,
        migrationEvidenceUri: evidence.migrationEvidenceUri,
        evidenceDigest: digest(observations),
        observedSchema: observedSchema(inspection),
      };
      await persist('reconciliation-evidence', {
        observations,
        inspection,
        proof,
      });
      const reconciled = maintenance.reconcileObserved(
        store,
        state,
        proof,
        observations,
      );
      return { state: reconciled, stateDigest: digest(reconciled) };
    }
    throw new Error(
      'Supported actions: status, adopt, enter, pause, resume, reconcile',
    );
  } finally {
    store.close?.();
  }
}
async function main() {
  trustedInvocation();
  assertControlRevision();
  const target = process.env.DELIVERY_TARGET;
  if (target === 'dev' && process.env.DELIVERY_DEV_ENABLED !== 'true')
    throw new Error(
      'Dev delivery is disabled; inventory/decommission are separate approved operations',
    );
  approvedEnvironment(`aws-${target}-maintenance`);
  const config = loadTargetConfig(target),
    ctx = {
      target,
      account: config.account,
      region: config.region,
      bucket: process.env.DELIVERY_BUCKET,
    };
  const action = process.argv[2],
    evidence = JSON.parse(process.env.DELIVERY_MAINTENANCE_EVIDENCE || '{}');
  const actual = action === 'status' ? config : discover(ctx, config);
  const result = await execute(action, ctx, actual, evidence);
  fs.mkdirSync('.delivery', { recursive: true });
  fs.writeFileSync(
    '.delivery/maintenance-result.json',
    JSON.stringify(result, null, 2) + '\n',
  );
  if (process.env.GITHUB_STEP_SUMMARY)
    fs.appendFileSync(
      process.env.GITHUB_STEP_SUMMARY,
      `## Maintenance ${action} / ${target}\n\nState: ${result.state?.state || 'see retained operation evidence'}\n\nState digest: ${result.stateDigest || 'see artifact'}\n\nA held operation remains excluded until verified reconciliation.\n`,
    );
  console.log(JSON.stringify(result));
}
if (require.main === module)
  main().catch((error) => {
    console.error(error.message);
    process.exitCode = 1;
  });
module.exports = { assertExpectedState, observedSchema, execute };

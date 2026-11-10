'use strict';
const test = require('node:test'),
  assert = require('node:assert/strict');
const { execute } = require('./maintenance-cli.cjs');
const { digest } = require('./contracts.cjs');
const f = require('./fixtures.cjs');
function fixture() {
  let state = {
    version: 1,
    target: 'prod',
    revision: 1,
    state: 'READY',
    lastVerified: {
      bundleDigest: f.hash,
      sourceSha: f.sha,
      verifiedAt: f.time,
    },
    observedSchema: { sha256: f.hash, observedAt: f.time, migrations: [] },
    active: null,
    holdReason: null,
  };
  const store = {
    read: () => ({ state, etag: String(state.revision) }),
    write: (value) => {
      state = value;
    },
    close: () => {},
  };
  return {
    store,
    get: () => state,
    ctx: {
      target: 'prod',
      account: '123456789012',
      region: 'eu-central-1',
      bucket: 'delivery',
    },
    config: {
      stackNames: ['application-prod-compute'],
      inspectorFunctionArn: 'inspector:1',
    },
    persist: () => {},
    inspectManual: () => ({ nativeWorkSettled: true }),
    readTask: () => ({ arn: 'task', captureDigest: f.hash, status: 'running' }),
    readRawTask: () => ({ ReplicationTaskSettings: '{}', TableMappings: '{}' }),
    verifyMaintenanceCompletion: async () => ({}),
  };
}
test('maintenance uses exact common exclusion and cannot enter twice', async () => {
  const p = fixture(),
    evidence = {
      expectedStateDigest: digest(p.get()),
      reason: 'Reviewed maintenance',
    };
  const entered = await execute('enter', p.ctx, p.config, evidence, p);
  assert.equal(entered.state.state, 'MAINTENANCE');
  assert.match(entered.state.active.executionId, /^maintenance:/);
  await assert.rejects(() =>
    execute(
      'enter',
      p.ctx,
      p.config,
      { ...evidence, expectedStateDigest: digest(p.get()) },
      p,
    ),
  );
});
test('manual reconciliation checks native work without inventing a pipeline and retains verified app baseline', async () => {
  const p = fixture();
  await execute(
    'enter',
    p.ctx,
    p.config,
    { expectedStateDigest: digest(p.get()), reason: 'Maintenance' },
    p,
  );
  let inspectionCalls = 0;
  const evidence = {
    expectedStateDigest: digest(p.get()),
    reason: 'Native work reviewed',
    operator: 'reviewer',
    migrationInvocationSettled: true,
    migrationEvidenceUri: 's3://delivery/maintenance-proof',
  };
  const deps = {
    ...p,
    inspectManual: () => ({
      executionId: p.get().active.executionId,
      nativeWorkSettled: false,
    }),
    inspectDatabase: () => {
      inspectionCalls++;
      return { ledger: [], schema: [] };
    },
  };
  await assert.rejects(() =>
    execute('reconcile', p.ctx, p.config, evidence, deps),
  );
  assert.equal(p.get().state, 'MAINTENANCE');
  assert.equal(inspectionCalls, 0);
  deps.inspectManual = () => ({
    executionId: p.get().active.executionId,
    nativeWorkSettled: true,
  });
  const result = await execute('reconcile', p.ctx, p.config, evidence, {
    ...deps,
    inspectDatabase: () => ({
      ledger: [{ version: 2, checksum: 'c'.repeat(96) }],
      schema: [],
    }),
  });
  assert.equal(result.state.state, 'READY');
  assert.equal(result.state.lastVerified.bundleDigest, f.hash);
  assert.equal(result.state.observedSchema.migrations[0].version, '2');
});
test('reconciling a held routine execution verifies from its admitted evidence, not maintenance records', async () => {
  const p = fixture();
  const execution = { executionId: 'native-execution', requestDigest: f.hash, bundleDigest: f.hash };
  p.store.write({ ...p.get(), revision: 2, state: 'NEEDS_ATTENTION', active: execution, holdReason: 'Health failed' });
  const calls = [];
  const result = await execute('reconcile', p.ctx, p.config, {
    expectedStateDigest: digest(p.get()),
    reason: 'Routine execution settled; health now passes',
    operator: 'reviewer',
    migrationInvocationSettled: true,
    migrationEvidenceUri: 's3://delivery/reports/executions/native-execution/migration-result.json',
  }, {
    ...p,
    inspectNative: () => ({ executionId: 'native-execution', nativeWorkSettled: true }),
    verifyMaintenanceCompletion: async () => { throw new Error('maintenance records do not exist'); },
    verifyNativeCompletion: async (_ctx, held) => { calls.push(held.active.executionId); return {}; },
    inspectDatabase: () => ({ ledger: [], schema: [] }),
  });
  assert.deepEqual(calls, ['native-execution']);
  assert.equal(result.state.state, 'READY');
  assert.equal(result.state.lastVerified.bundleDigest, f.hash);
});
test('admitted evidence must belong to the held execution and bundle', async () => {
  const { verifyNativeCompletion } = require('./maintenance.cjs');
  const { createHash } = require('node:crypto');
  const held = { active: { executionId: 'native-execution', requestDigest: f.hash, bundleDigest: 'b'.repeat(64) } };
  const inventoryBody = JSON.stringify({
    candidateBundleDigest: 'b'.repeat(64),
    dmsBaseline: { arn: 'task', status: 'running' },
    aws: { activation: [{ id: 'mapping', source: 'arn:aws:kinesis:stream' }] },
  });
  const report = {
    evidence: [{ kind: 'INVENTORY', uri: 's3://delivery/reports/evidence/i.json?versionId=v1',
      sha256: createHash('sha256').update(inventoryBody).digest('hex') }],
  };
  const objects = (admissionExecution) => ({
    'reports/executions/native-execution/admission.json': {
      execution: admissionExecution, reportDigest: digest(report), startedAt: f.time,
    },
    [`reports/${digest(report)}.json`]: report,
  });
  const store = (records) => ({
    readJson: (_bucket, key) => records[key],
    readObject: () => ({ body: inventoryBody }),
  });
  const ctx = { bucket: 'delivery' };
  const seen = [];
  const verifyCurrentHealth = async (_ctx, _config, dms, startedAt, options) => {
    seen.push({ dms, startedAt, router: options.routerBaseline.id });
    return { status: 'PASS' };
  };
  await verifyNativeCompletion(ctx, held, {}, null, null, { store: store(objects(held.active)), verifyCurrentHealth });
  assert.deepEqual(seen, [{ dms: { arn: 'task', status: 'running' }, startedAt: f.time, router: 'mapping' }]);
  await assert.rejects(() => verifyNativeCompletion(ctx, held, {}, null, null, {
    store: store(objects({ ...held.active, executionId: 'other' })), verifyCurrentHealth }), /another execution/);
  await assert.rejects(() => verifyNativeCompletion(ctx, { active: { ...held.active, bundleDigest: 'c'.repeat(64) } }, {}, null, null, {
    store: store(objects({ ...held.active, bundleDigest: 'c'.repeat(64) })), verifyCurrentHealth }), /another bundle/);
});

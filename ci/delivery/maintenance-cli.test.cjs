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

'use strict';
const test = require('node:test'),
  assert = require('node:assert/strict');
const { adopt } = require('./adoption.cjs');
const { digest } = require('./contracts.cjs');
const f = require('./fixtures.cjs');
function fixture() {
  let state = null,
    revision = 0;
  const bundle = f.bundle(),
    ledger = bundle.migrations.map((row) => ({
      version: Number(row.version),
      checksum: row.checksum,
      success: true,
    }));
  const store = {
    read: () => (state ? { state, etag: String(revision) } : null),
    write: (value) => {
      state = value;
      revision++;
    },
  };
  const options = {
    readJson: () => bundle,
    validateObjects: () => {},
    inspectDatabase: () => ({ ledger, schema: [] }),
    candidateTemplates: () => ({}),
    verifyStacks: () => {},
    cumulativeSource: () => ({
      migrations: ledger.map((row) => ({ ...row, sql: 'SELECT 1' })),
    }),
    snapshots: async () => ({ after: [] }),
    inspectNative: () => ({ nativeWorkSettled: true }),
    api: () => ({
      ReplicationTasks: [
        {
          ReplicationTaskArn: 'task',
          Status: 'running',
          ReplicationTaskSettings: '{}',
          TableMappings: '{}',
        },
      ],
    }),
    verifyCurrentHealth: async () => ({ status: 'PASS' }),
    persist: () => {},
  };
  return {
    store,
    options,
    ctx: {
      target: 'prod',
      account: '123456789012',
      region: 'eu-central-1',
      bucket: 'bundles',
    },
    config: {
      inspectorFunctionArn: 'inspector:1',
      dmsTaskArn: 'task',
      stackNames: [],
    },
    evidence: {
      bundleDigest: digest(bundle),
      reason: 'Verified baseline adoption',
      expectedDmsStatus: 'running',
    },
    state: () => state,
  };
}
test('first verified baseline is recorded only after deployed artifacts schema and native settlement pass', async () => {
  const p = fixture();
  const result = await adopt(p.ctx, p.config, p.evidence, p.store, p.options);
  assert.equal(result.state.state, 'READY');
  assert.equal(result.state.lastVerified.bundleDigest, p.evidence.bundleDigest);
  await assert.rejects(() =>
    adopt(p.ctx, p.config, p.evidence, p.store, p.options),
  );
});
test('failed native adoption retains explicit hold and never invents a verified baseline', async () => {
  const p = fixture();
  p.options.inspectNative = () => ({ nativeWorkSettled: false });
  await assert.rejects(() =>
    adopt(p.ctx, p.config, p.evidence, p.store, p.options),
  );
  assert.equal(p.state().state, 'MAINTENANCE');
  assert.equal(p.state().lastVerified, null);
});

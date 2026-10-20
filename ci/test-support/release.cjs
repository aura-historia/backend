'use strict';
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const os = require('node:os');
const { hash } = require('../release/io.cjs');
const { Store } = require('../release/state.cjs');
const { Coordinator } = require('../release/coordinator.cjs');
const dms = require('../release/dms.cjs');
const stacks = require('../release/stacks.cjs');
const sha = 'a'.repeat(40),
  baseline = 'b'.repeat(40);
const ctx = {
  stage: 'dev',
  sha,
  region: 'eu-central-1',
  account: '123456789012',
  bucket: 'release-state',
  key: 'stages/123456789012/eu-central-1/dev.json',
  run: '100',
  attempt: '1',
  repository: 'aura-historia/backend',
  budget: 10800,
  ref: sha,
};
const arn = `arn:aws:dms:${ctx.region}:${ctx.account}:task:EXACTTASK`;
const clone = (value) => structuredClone(value);
function task(status = 'running') {
  return {
    ReplicationTaskArn: arn,
    SourceEndpointArn: 'source',
    TargetEndpointArn: 'target',
    ReplicationInstanceArn: 'instance',
    Status: status,
    StopReason: 'Stop Reason NORMAL',
    MigrationType: 'cdc',
    RecoveryCheckpoint: 'checkpoint:V1#original',
    ReplicationTaskStats: { StartDate: '2026-10-01T00:00:00Z', TablesErrored: 0 },
  };
}
function memory(initial) {
  let value = initial ? clone(initial) : null,
    etag = value ? '"1"' : null;
  const writes = [];
  const api = (_ctx, service, op, args) => {
    assert.equal(service, 's3api');
    const arg = (key) => args[args.indexOf(key) + 1];
    assert.equal(arg('--expected-bucket-owner'), ctx.account);
    assert.equal(arg('--bucket'), ctx.bucket);
    assert.equal(arg('--key'), ctx.key);
    if (op === 'get-object') {
      if (!value) throw Object.assign(new Error('missing'), { code: 'NoSuchKey' });
      fs.writeFileSync(args.at(-1), JSON.stringify(value));
      return { ETag: etag };
    }
    assert.equal(op, 'put-object');
    if (value) assert.equal(arg('--if-match'), etag, 'stale writer must be fenced');
    else assert.equal(arg('--if-none-match'), '*');
    value = JSON.parse(fs.readFileSync(arg('--body'), 'utf8'));
    etag = `"${value.revision}"`;
    writes.push(clone(value));
    return { ETag: etag };
  };
  return { store: new Store(ctx, api), writes, value: () => clone(value), api };
}
const empty = () => ({
  version: 1,
  account: ctx.account,
  region: ctx.region,
  stage: ctx.stage,
  revision: 1,
  completed: null,
  active: null,
});
function harness(original = 'running', pending = true) {
  const mem = memory(empty()),
    events = [],
    live = {},
    targets = {};
  const f = { mem, events, fail: null, currentTask: task(original), ctx: { ...ctx } };
  for (const name of stacks.names(ctx)) {
    live[name] = { template: { Resources: { Old: {} } } };
    targets[name] = { Resources: { New: { name } } };
  }
  f.makePlan = () => ({
    version: 1,
    alreadyComplete: f.alreadyComplete || false,
    sha,
    baseline,
    run: f.ctx.run,
    attempt: f.ctx.attempt,
    stateEtag: mem.store.read().etag,
    requirements: { sql: pending ? ['migrations/pending.sql'] : [] },
    activation: { CdcRouterEnabled: 'true', PeriodicMatcherEnabled: 'false' },
    runtimeActivation: {},
    captureConfig: hash('capture'),
    task: { arn },
    targets: Object.fromEntries(Object.entries(targets).map(([name, value]) => [name, hash(value)])),
  });
  const effect = (event) => {
    events.push(event);
    if (f.fail === event) {
      f.fail = null;
      throw new Error(`interrupted ${event}`);
    }
  };
  const options = {
    plan: () => f.makePlan(),
    approval: () => ({ actor: 'reviewer' }),
    canContinue: () => {},
    monitor: () => {},
    stacks: {
      inspect: () => live,
      normalized: (x) => x,
      runtimeActivation: () => ({}),
      snapshot: () => clone(live),
      settle: async (_ctx, name) => effect(`settle:${name}`),
      deployed: (_ctx, name) => hash(live[name].template) === hash(targets[name]),
      deploy: (_ctx, name) => {
        assert.equal(mem.value().active.phase, `DEPLOY_${name.toUpperCase()}`);
        assert.equal(f.currentTask.Status, 'stopped');
        live[name].template = targets[name];
        effect(`deploy:${name}`);
      },
      verify: () => {
        for (const name of stacks.names(ctx)) assert.deepEqual(live[name].template, targets[name]);
        effect('verify');
      },
    },
    dms: {
      read: () => f.currentTask,
      admission: dms.admission,
      evidence: dms.evidence,
      sameTask: dms.sameTask,
      pause: async () => {
        assert.equal(mem.value().active.phase, 'PAUSING');
        assert.equal(mem.value().active.pauseOwned, original === 'running');
        f.currentTask.Status = 'stopped';
        effect('pause');
        return dms.evidence(f.currentTask);
      },
      resume: async () => {
        assert.ok(mem.value().active.completed.includes('DEPLOYED'));
        assert.equal(mem.value().active.phase, 'RESUMING');
        f.currentTask.Status = 'running';
        effect('resume');
        return dms.evidence(f.currentTask);
      },
      health: async () => {
        effect('health');
        return { progress: true };
      },
    },
    command: (_program, args, _timeout, env) => {
      assert.deepEqual(args, ['ci/invoke-operation.sh', 'migrate']);
      assert.equal(env.RELEASE_ID, mem.value().active.id);
      assert.ok(mem.value().active.completed.includes('DEPLOY_INITIALIZE'));
      effect('migrate');
    },
  };
  f.run = () => new Coordinator(f.ctx, mem.store, options).run(f.makePlan(), targets, {});
  f.options = options;
  return f;
}

module.exports = { sha, baseline, ctx, arn, clone, task, memory, empty, harness };

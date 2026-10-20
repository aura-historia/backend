'use strict';
const test = require('node:test');
const assert = require('node:assert/strict');
const { hash, command, context } = require('./release/io.cjs');
const { Store, assertAvailable, assertOwner, owner } = require('./release/state.cjs');
const { Coordinator } = require('./release/coordinator.cjs');
const { protection, approval, canContinue } = require('./release/github.cjs');
const { assertPhase } = require('./release.cjs');
const { sha, baseline, ctx, memory, harness } = require('./test-support/release.cjs');
for (const pending of [true, false])
  test(`forward release always pauses and invokes exact migrator (pending SQL=${pending})`, async () => {
    const f = harness('running', pending);
    await f.run();
    assert.deepEqual(f.events, [
      'pause',
      'deploy:network',
      'deploy:data',
      'deploy:initialize',
      'migrate',
      'deploy:compute',
      'deploy:api',
      'verify',
      'resume',
      'health',
    ]);
    assert.equal(f.mem.value().active, null);
    assert.equal(f.mem.value().completed.sha, sha);
    assert.equal(f.mem.value().completed.evidence.MIGRATED.result, 'ready');
    assert.equal(f.mem.value().completed.activation.PeriodicMatcherEnabled, 'false');
  });
test('intentionally stopped task is never resumed and is exempt from running health', async () => {
  const f = harness('stopped');
  await f.run();
  assert.ok(!f.events.includes('resume'));
  assert.ok(!f.events.includes('health'));
  assert.equal(f.mem.value().completed.finalDmsState, 'stopped');
});
for (const failure of [
  'pause',
  'deploy:network',
  'deploy:data',
  'deploy:initialize',
  'migrate',
  'deploy:compute',
  'deploy:api',
  'verify',
  'resume',
  'health',
]) {
  test(`retain progress and safely continue after ${failure}`, async () => {
    const f = harness();
    f.fail = failure;
    await assert.rejects(f.run(), /interrupted/);
    const interrupted = f.mem.value().active;
    assert.equal(interrupted.sha, sha);
    assert.equal(interrupted.pauseOwned, true);
    if (!['resume', 'health'].includes(failure)) assert.ok(!f.events.includes('resume'));
    if (failure === 'migrate') assert.ok(!f.events.includes('deploy:compute'));
    const before = [...f.events];
    f.ctx.run = '101';
    await f.run();
    assert.equal(f.mem.value().active, null);
    assert.equal(f.mem.value().completed.priorDmsState, 'running');
    if (['deploy:compute', 'deploy:api', 'verify', 'resume', 'health'].includes(failure))
      assert.equal(f.events.filter((e) => e === 'migrate').length, 1);
    if (failure === 'health') assert.equal(f.events.filter((e) => e === 'resume').length, 1);
    assert.ok(f.events.length > before.length);
  });
}
test('a stale approved plan cannot acquire stage ownership', async () => {
  const f = harness();
  const p = f.makePlan();
  const snapshot = f.mem.store.read();
  f.mem.store.write(snapshot, snapshot.state);
  await assert.rejects(new Coordinator(ctx, f.mem.store, f.options).run(p, {}, {}), /changed after planning/);
  assert.deepEqual(f.events, []);
});
test('changed live baseline after approval cannot pause capture', async () => {
  const f = harness();
  f.options.plan = () => ({ ...f.makePlan(), baseline: sha });
  await assert.rejects(f.run(), /changed after approval/);
  assert.deepEqual(f.events, []);
});
test('approval failure leaves no ownership or DMS effect', async () => {
  const f = harness();
  f.options.approval = () => {
    throw new Error('No protected approval');
  };
  await assert.rejects(f.run(), /approval/);
  assert.equal(f.mem.value().active, null);
  assert.deepEqual(f.events, []);
});
test('conditional S3 writes fence stale owners and fail closed after an ambiguous response', () => {
  const m = memory();
  const a = m.store.read(),
    b = m.store.read();
  const first = m.store.write(a, a.state);
  assert.equal(first.state.revision, 1);
  assert.throws(() => m.store.write(b, b.state), /rejected or its result is unknown/);
  const ambiguous = new Store(ctx, (...args) => {
    const value = m.api(...args);
    if (args[2] === 'put-object') throw new Error('response lost');
    return value;
  });
  assert.throws(() => ambiguous.write(first, first.state), /unknown/);
  assert.equal(m.store.read().state.revision, 2);
});
test('another SHA and every exceptional operation are blocked by an unfinished release', () => {
  const active = { kind: 'RELEASE', sha, phase: 'MIGRATING' };
  assert.throws(
    () => assertAvailable({ active }, { ...ctx, sha: baseline }, 'release'),
    /incomplete RELEASE/,
  );
  for (const kind of ['all', 'foundation', 'migrate', 'initialize', 'adopt'])
    assert.throws(() => assertAvailable({ active }, ctx, kind), /incomplete RELEASE/);
});
test('a fenced or unowned helper invocation cannot mutate', () => {
  assert.throws(() => assertOwner({ state: { active: null } }, ctx, 'fake'), /ownership changed/);
  const store = {
    read: () => ({
      state: { active: { id: 'id', kind: 'RELEASE', sha, owner: owner(ctx), phase: 'PAUSED' } },
    }),
  };
  const prior = process.env.RELEASE_ID;
  process.env.RELEASE_ID = 'id';
  try {
    for (const phase of ['migrate', 'initialize', 'exception', 'compute'])
      assert.throws(() => assertPhase(ctx, store, phase), /not authorized/);
  } finally {
    if (prior === undefined) delete process.env.RELEASE_ID;
    else process.env.RELEASE_ID = prior;
  }
});
test('same-SHA takeover requires a completed previous GitHub attempt, never lease expiry', () => {
  const active = { owner: { ...owner(ctx), run: '90' } };
  assert.throws(() => canContinue(ctx, active, () => ({ status: 'in_progress' })), /still active/);
  canContinue(ctx, active, () => ({ status: 'completed' }));
  assert.throws(
    () => canContinue(ctx, { owner: { ...active.owner, repository: 'other/repo' } }, () => assert.fail()),
    /different repository/,
  );
});
const environment = {
  id: 10,
  name: 'aws-dev',
  can_admins_bypass: false,
  protection_rules: [{ type: 'required_reviewers', reviewers: [{ id: 7 }], prevent_self_review: true }],
};
test('configured environment review policy can allow the operator to approve their own release', () => {
  const configured = {
    ...environment,
    protection_rules: [{ ...environment.protection_rules[0], prevent_self_review: false }],
  };
  assert.equal(
    protection(ctx, () => configured),
    configured,
  );
  const api = (_ctx, endpoint) =>
    endpoint.startsWith('environments/')
      ? configured
      : [{ state: 'approved', user: { login: 'operator' }, environments: [{ id: configured.id }] }];
  assert.equal(approval(ctx, { run: ctx.run, attempt: ctx.attempt }, api).actor, 'operator');
});
test('OpenSearch confirmation is bound to the plan, run, attempt and protected stage approval', () => {
  const p = { run: ctx.run, attempt: ctx.attempt, requirements: { id: 'prerequisite' } };
  const api = (_ctx, endpoint) =>
    endpoint.startsWith('environments/')
      ? environment
      : [{ state: 'approved', user: { login: 'reviewer' }, environments: [{ id: 10 }] }];
  const evidence = approval(ctx, p, api);
  assert.equal(evidence.plan, hash(p));
  assert.equal(evidence.prerequisite, 'prerequisite');
  assert.throws(() => approval({ ...ctx, attempt: '2' }, p, api), /another run\/attempt/);
  assert.throws(
    () => approval(ctx, p, (_ctx, endpoint) => (endpoint.startsWith('environments/') ? environment : [])),
    /No protected/,
  );
  assert.throws(
    () => protection(ctx, () => ({ ...environment, protection_rules: [] })),
    /required reviewers/,
  );
  assert.throws(
    () => protection(ctx, () => ({ ...environment, can_admins_bypass: true })),
    /administrator bypass/,
  );
});
test('command errors never expose raw provider output', () => {
  assert.throws(
    () =>
      command(process.execPath, ['-e', 'process.stderr.write("PRIVATE_PROVIDER_PAYLOAD");process.exit(1)']),
    (error) => !error.message.includes('PRIVATE_PROVIDER_PAYLOAD'),
  );
});
test('invalid account, stage and run inputs cannot construct operational context', () => {
  assert.throws(() => context({}), /Set valid/);
});

for (const phase of ['PREPARED', 'PAUSING', 'DEPLOY_DATA', 'MIGRATING', 'RESUMING', 'VERIFYING']) {
  test(`runner loss after persisting ${phase} intent is recoverable`, async () => {
    const f = harness();
    const write = f.mem.store.write.bind(f.mem.store);
    let lost = false;
    f.mem.store.write = (previous, next) => {
      const result = write(previous, next);
      if (!lost && next.active?.phase === phase) {
        lost = true;
        throw new Error('State write response lost');
      }
      return result;
    };
    await assert.rejects(f.run(), /response lost/);
    const originalId = f.mem.value().active.id;
    f.ctx.run = '101';
    await f.run();
    assert.equal(f.mem.value().completed.id, originalId);
    assert.equal(f.mem.value().completed.priorDmsState, 'running');
  });
}
for (const phase of [
  'PAUSED',
  'DEPLOY_INITIALIZE',
  'MIGRATED',
  'DEPLOY_COMPUTE',
  'DEPLOYED',
  'CDC_VERIFIED',
]) {
  test(`lost confirmation write after ${phase} preserves completed evidence`, async () => {
    const f = harness();
    const write = f.mem.store.write.bind(f.mem.store);
    let lost = false;
    f.mem.store.write = (previous, next) => {
      const result = write(previous, next);
      if (!lost && next.active?.completed.includes(phase)) {
        lost = true;
        throw new Error('State write response lost');
      }
      return result;
    };
    await assert.rejects(f.run(), /response lost/);
    f.ctx.run = '101';
    await f.run();
    assert.equal(f.events.filter((e) => e === 'migrate').length, 1);
    assert.equal(f.mem.value().active, null);
  });
}
test('a release whose owned pause exceeds the configured budget cannot deploy or force a resume', async () => {
  const f = harness();
  f.fail = 'pause';
  await assert.rejects(f.run());
  const current = f.mem.store.read();
  f.mem.store.write(current, {
    ...current.state,
    active: { ...current.state.active, pauseAt: '2000-01-01T00:00:00Z' },
  });
  f.events.length = 0;
  f.ctx.run = '101';
  await assert.rejects(f.run(), /exceeded its WAL budget/);
  assert.ok(!f.events.some((e) => e.startsWith('deploy:') || e === 'resume'));
  assert.equal(f.mem.value().active.pauseOwned, true);
});

test('retrying a confirmed complete release verifies state without repeating operational effects', async () => {
  const f = harness();
  await f.run();
  const revision = f.mem.value().revision;
  f.alreadyComplete = true;
  f.events.length = 0;
  await f.run();
  assert.deepEqual(f.events, ['verify']);
  assert.equal(f.mem.value().revision, revision);
  assert.equal(f.mem.value().active, null);
});

'use strict';
const test = require('node:test');
const assert = require('node:assert/strict');
const dms = require('./release/dms.cjs');
const { monitor } = require('./release/plan.cjs');
const { observation } = require('../infra/src/resources/release-monitor/index.js');
const { task, ctx, arn, clone } = require('./test-support/release.cjs');
function dmsApi(current, effects = {}) {
  const calls = [];
  const api = (_ctx, service, operation, args) => {
    assert.equal(service, 'dms');
    calls.push({ operation, args });
    if (operation === 'describe-replication-tasks') {
      assert.deepEqual(args, ['--filters', `Name=replication-task-arn,Values=${arn}`, '--without-settings']);
      return { ReplicationTasks: [clone(current)] };
    }
    if (operation === 'stop-replication-task') {
      assert.deepEqual(args, ['--replication-task-arn', arn]);
      if (!effects.stuck) current.Status = 'stopped';
    } else if (operation === 'start-replication-task') {
      assert.deepEqual(args, [
        '--replication-task-arn',
        arn,
        '--start-replication-task-type',
        'resume-processing',
      ]);
      if (!effects.stuck) current.Status = 'running';
    } else assert.fail(`unexpected ${operation}`);
    if (effects.lost) throw new Error('PRIVATE PROVIDER PAYLOAD');
    return {};
  };
  return { api, calls, delay: async () => {}, attempts: 2 };
}
test('lost stop and resume responses reconcile the exact task without resetting capture', async () => {
  const live = task(),
    recorded = dms.evidence(live),
    options = dmsApi(live, { lost: true });
  assert.equal((await dms.pause(ctx, recorded, options)).status, 'stopped');
  assert.equal((await dms.resume(ctx, recorded, options)).status, 'running');
  assert.equal(options.calls.filter((c) => c.operation === 'stop-replication-task').length, 1);
  assert.equal(options.calls.filter((c) => c.operation === 'start-replication-task').length, 1);
  await dms.resume(ctx, recorded, options);
  assert.equal(options.calls.filter((c) => c.operation === 'start-replication-task').length, 1);
});
test('stop must actually stabilize and every wait has a deadline', async () => {
  const live = task(),
    options = dmsApi(live, { stuck: true });
  await assert.rejects(dms.pause(ctx, dms.evidence(live), options), /deadline/);
  assert.equal(options.calls.length, 4);
});
for (const [label, change] of [
  [
    'first start',
    (t) => {
      delete t.ReplicationTaskStats.StartDate;
    },
  ],
  [
    'failed',
    (t) => {
      t.Status = 'failed';
    },
  ],
  [
    'missing checkpoint',
    (t) => {
      delete t.RecoveryCheckpoint;
    },
  ],
  [
    'invalid checkpoint',
    (t) => {
      t.RecoveryCheckpoint = 'somewhere';
    },
  ],
  [
    'table errors',
    (t) => {
      t.ReplicationTaskStats.TablesErrored = 1;
    },
  ],
  [
    'replaced',
    (t) => {
      t.ReplicationTaskArn += 'OTHER';
    },
  ],
])
  test(`reject ${label} before stopping DMS`, () => {
    const live = task();
    change(live);
    assert.throws(() => dms.read(ctx, arn, dmsApi(live).api));
  });
test('unexplained lifecycle transitions and topology changes block admission', () => {
  for (const status of ['ready', 'starting', 'stopping', 'modifying'])
    assert.throws(() => dms.admission(task(status), null), /no recorded operation/);
  assert.throws(
    () => dms.sameTask({ ...task(), SourceEndpointArn: 'other' }, dms.evidence(task())),
    /source changed/,
  );
  assert.throws(
    () => dms.admission({ ...task('stopped'), StopReason: 'RECOVERABLE_ERROR' }, null),
    /intentional/,
  );
  dms.admission(task('stopped'), { phase: 'PAUSING', dms: dms.evidence(task()) });
});
function healthApi({ idle = false, missing = false, errors = false, lag = 0, checkpoint = true } = {}) {
  return (_ctx, service, operation, args) => {
    if (operation === 'describe-replication-instances')
      return {
        ReplicationInstances: [
          { ReplicationInstanceArn: 'instance', ReplicationInstanceIdentifier: 'physical' },
        ],
      };
    if (operation === 'describe-replication-tasks')
      return {
        ReplicationTasks: [
          {
            ...task(),
            RecoveryCheckpoint: checkpoint ? 'checkpoint:V1#advanced' : task().RecoveryCheckpoint,
          },
        ],
      };
    if (operation === 'describe-table-statistics')
      return { TableStatistics: [{ TableState: errors ? 'Table error' : 'Table completed' }] };
    assert.equal(service, 'cloudwatch');
    assert.equal(operation, 'get-metric-statistics');
    const name = args[args.indexOf('--metric-name') + 1],
      statistic = args[args.indexOf('--statistics') + 1];
    if (name === 'IteratorAge' && idle && lag === 0) return { Datapoints: [] };
    return {
      Datapoints: missing
        ? []
        : [
            {
              Timestamp: new Date().toISOString(),
              [statistic]:
                name === 'IteratorAge'
                  ? lag
                  : name.includes('Throughput') || name === 'IncomingRecords'
                    ? Number(!idle)
                    : 0,
            },
          ],
    };
  };
}
const streamStacks = {
  data: { resources: [{ ResourceType: 'AWS::Kinesis::Stream', PhysicalResourceId: 'stage-stream' }] },
};
for (const idle of [false, true])
  test(`bounded CDC verification handles ${idle ? 'idle source with no router invocations' : 'checkpoint progress'}`, async () => {
    const evidence = await dms.health(
      ctx,
      dms.evidence(task()),
      'true',
      new Date(Date.now() - 10000).toISOString(),
      streamStacks,
      { api: healthApi({ idle, checkpoint: !idle }), attempts: 1 },
    );
    assert.equal(evidence.idleSource, idle);
  });
for (const scenario of [
  { missing: true },
  { errors: true },
  { lag: 61000 },
  { lag: 61000, idle: true },
  { checkpoint: false },
])
  test(`CDC verification retains failure for ${JSON.stringify(scenario)}`, async () => {
    await assert.rejects(
      dms.health(
        ctx,
        dms.evidence(task()),
        'true',
        new Date(Date.now() - 10000).toISOString(),
        streamStacks,
        { api: healthApi(scenario), delay: async () => {}, attempts: 1 },
      ),
      /unverified|exceeded/,
    );
  });
test('disabled router is preserved and exempt only from router lag, not capture health', async () => {
  await dms.health(
    ctx,
    dms.evidence(task()),
    'false',
    new Date(Date.now() - 10000).toISOString(),
    streamStacks,
    { api: healthApi({ lag: 999999 }), attempts: 1 },
  );
});
test('runner-independent monitor alerts on overdue pauses without any DMS capability', () => {
  const now = Date.now();
  const state = {
    version: 1,
    active: { pauseOwned: true, pauseAt: new Date(now - 120000).toISOString(), pauseBudgetSeconds: 60 },
  };
  assert.equal(observation(state, now).overdue, 1);
  assert.equal(
    observation({ ...state, active: { ...state.active, resumedAt: new Date().toISOString() } }, now).overdue,
    0,
  );
  assert.equal(observation({ version: 1, active: null }, now).overdue, 0);
  assert.throws(() => observation({ version: 9 }, now), /Unsupported/);
});
test('monitor admission blocks missing alarms, disabled notifications and missing observations', () => {
  for (const MetricAlarms of [
    [],
    [{ StateValue: 'OK', ActionsEnabled: false }],
    [{ StateValue: 'INSUFFICIENT_DATA' }, { StateValue: 'OK' }],
  ])
    assert.throws(() => monitor(ctx, null, () => ({ MetricAlarms })), /monitoring/);
});

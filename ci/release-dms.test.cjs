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
function healthApi({
  idle = false,
  missing = false,
  errors = false,
  lag = 0,
  checkpoint = true,
  noWriteMetric = false,
  readerAge = 0,
  readRecords = 0,
  successfulReads = 30,
  staleReads = false,
  mappingState = 'Enabled',
  foreignMapping = false,
} = {}) {
  return (_ctx, service, operation, args) => {
    if (operation === 'get-event-source-mapping') {
      assert.equal(service, 'lambda');
      assert.deepEqual(args, ['--uuid', 'stage-router-mapping']);
      return {
        UUID: 'stage-router-mapping',
        State: mappingState,
        FunctionArn: `arn:aws:lambda:${ctx.region}:${ctx.account}:function:cdc-router-lambda-${ctx.stage}:20`,
        EventSourceArn: `arn:aws:kinesis:${ctx.region}:${ctx.account}:stream/${foreignMapping ? 'foreign-stream' : 'stage-stream'}`,
      };
    }
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
    if (name === 'IncomingRecords' && noWriteMetric) return { Datapoints: [] };
    const reads = {
      'GetRecords.IteratorAgeMilliseconds': readerAge,
      'GetRecords.Records': readRecords,
      'GetRecords.Success': successfulReads,
    };
    if (Object.hasOwn(reads, name))
      return {
        Datapoints:
          reads[name] === null
            ? []
            : [
                {
                  Timestamp: new Date(Date.now() - (staleReads ? 600000 : 0)).toISOString(),
                  [statistic]: reads[name],
                },
              ],
      };
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
  compute: {
    stack: {
      Outputs: [
        {
          ExportName: `aura-historia-cdc-router-event-source-mapping-id-${ctx.stage}`,
          OutputValue: 'stage-router-mapping',
        },
      ],
    },
    resources: [
      { ResourceType: 'AWS::Lambda::EventSourceMapping', PhysicalResourceId: 'stage-router-mapping' },
    ],
  },
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
test('idle adoption completes with fresh caught-up empty polls when write and invocation metrics are absent', async () => {
  const messages = [];
  const result = await dms.health(
    ctx,
    dms.evidence(task()),
    'true',
    new Date(Date.now() - 300000).toISOString(),
    streamStacks,
    {
      api: healthApi({ idle: true, checkpoint: false, noWriteMetric: true }),
      delay: async () => assert.fail('Healthy idle stream must not wait'),
      attempts: 1,
      report: (message) => messages.push(message),
    },
  );
  assert.equal(result.idleSource, true);
  assert.equal(result.idleStream, true);
  assert.equal(result.routerAgeMs, null);
  assert.equal(result.streamReadAgeMs, 0);
  assert.equal(result.streamReadRecords, 0);
  assert.equal(result.streamSuccessfulReads, 30);
  assert.ok(messages.some((message) => message.includes('Healthy') && message.includes('router=missing')));
});
for (const scenario of [
  { readerAge: 1 },
  { readRecords: 1 },
  { successfulReads: 0 },
  { readerAge: null },
  { readRecords: null },
  { successfulReads: null },
  { staleReads: true },
])
  test(`idle fallback rejects missing, stale or nonempty read evidence ${JSON.stringify(scenario)}`, async () => {
    const messages = [];
    await assert.rejects(
      dms.health(
        ctx,
        dms.evidence(task()),
        'true',
        new Date(Date.now() - 300000).toISOString(),
        streamStacks,
        {
          api: healthApi({ idle: true, checkpoint: false, noWriteMetric: true, ...scenario }),
          delay: async () => {},
          attempts: 1,
          report: (message) => messages.push(message),
        },
      ),
      /exceeded.*fresh router age or verified idle stream reads/,
    );
    assert.ok(
      messages.some((message) => message.includes('Waiting for') && message.includes('idleStream=missing')),
    );
  });
for (const scenario of [{ mappingState: 'Disabled' }, { foreignMapping: true }])
  test(`health verification rejects a disabled or foreign CDC router ${JSON.stringify(scenario)}`, async () => {
    await assert.rejects(
      dms.health(
        ctx,
        dms.evidence(task()),
        'true',
        new Date(Date.now() - 300000).toISOString(),
        streamStacks,
        {
          api: healthApi({ idle: true, noWriteMetric: true, ...scenario }),
          attempts: 1,
          report: () => {},
        },
      ),
      /router mapping/,
    );
  });
test('health verification requires the router mapping export to be owned by the compute stack', async () => {
  const live = clone(streamStacks);
  live.compute.resources = [];
  await assert.rejects(
    dms.health(ctx, dms.evidence(task()), 'true', new Date(Date.now() - 300000).toISOString(), live, {
      api: healthApi({ idle: true, noWriteMetric: true }),
      attempts: 1,
      report: () => {},
    }),
    /stage-owned CDC router mapping/,
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

'use strict';
const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const os = require('node:os');
const { execFileSync } = require('node:child_process');
const { hash } = require('./release/io.cjs');
const { requirements } = require('./release/requirements.cjs');
const { plan } = require('./release/plan.cjs');
const stacks = require('./release/stacks.cjs');
const dms = require('./release/dms.cjs');
const { loadCatalog } = require('./container-images.cjs');
const { sha, baseline, ctx, arn, task, clone, empty } = require('./test-support/release.cjs');
function repository(t) {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'release-requirements-'));
  t.after(() => fs.rmSync(dir, { recursive: true, force: true }));
  const git = (args) =>
    execFileSync('git', args, { cwd: dir, encoding: 'utf8', stdio: ['ignore', 'pipe', 'pipe'] });
  git(['init', '-q']);
  const write = (file, contents) => {
    fs.mkdirSync(path.dirname(path.join(dir, file)), { recursive: true });
    fs.writeFileSync(path.join(dir, file), contents);
  };
  const commit = () => {
    git(['add', '.']);
    git(['-c', 'user.name=Fixture', '-c', 'user.email=fixture@example.invalid', 'commit', '-qm', 'fixture']);
    return git(['rev-parse', 'HEAD']).trim();
  };
  write('opensearch/mapping.json', '{"version":1}');
  const base = commit();
  const add = (id, classification = 'manual', asset = true, extra = {}) => {
    const file = 'opensearch/mapping.json';
    let assets = {};
    if (asset) {
      const from = git(['rev-parse', `HEAD:${file}`]).trim();
      write(file, JSON.stringify({ id }));
      const to = git(['hash-object', file]).trim();
      assets = { [file]: { from, to } };
    }
    const declaration = {
      version: 1,
      id,
      kind: 'OPENSEARCH',
      classification: classification.toUpperCase(),
      reason: 'Reviewed compatibility',
      runbook: 'docs/opensearch-stage.md#release-and-schema-change-contract',
      action: 'Apply additive mapping',
      validation: 'Check the stage mapping',
      assets,
      ...extra,
    };
    write(`releases/requirements/${id}.json`, JSON.stringify(declaration));
    return commit();
  };
  return { git, base, write, commit, add };
}
test('classifies exact OpenSearch differences across skipped commits and retains each manual prerequisite', (t) => {
  const r = repository(t);
  r.add('first-change');
  const target = r.add('second-change', 'none');
  const result = requirements(r.base, target, r.git);
  assert.equal(result.entries.length, 2);
  assert.equal(result.manual.length, 1);
  assert.deepEqual(result.search, ['opensearch/mapping.json']);
});
test('application-only OpenSearch declarations do not require an asset change', (t) => {
  const r = repository(t);
  const target = r.add('application-prerequisite', 'manual', false);
  const result = requirements(r.base, target, r.git);
  assert.equal(result.manual.length, 1);
  assert.deepEqual(result.search, []);
});
test('unclassified assets, unsupported cutovers and missing deployed prerequisites block before mutation', (t) => {
  const r = repository(t);
  r.write('opensearch/mapping.json', '{}');
  const unclassified = r.commit();
  assert.throws(() => requirements(r.base, unclassified, r.git), /Unclassified/);
  const cutover = r.add('special-cutover', 'unsupported');
  assert.throws(() => requirements(unclassified, cutover, r.git), /dedicated operation/);
  const target = r.add('new-prerequisite', 'none', false, { requiresDeployed: sha });
  assert.throws(() => requirements(cutover, target, r.git), /deployed prerequisite/);
});
test('changed or removed requirements cannot erase prerequisite history', (t) => {
  const r = repository(t);
  r.add('durable-requirement');
  r.write('releases/requirements/durable-requirement.json', '{}');
  const target = r.commit();
  assert.throws(() => requirements(r.base, target, r.git), /append-only/);
});
test('migration additions require an explicit running-code and retained-work compatibility declaration', (t) => {
  const r = repository(t);
  const file = 'migrations/20261020120000_change.sql';
  r.write(file, 'ALTER TABLE example ADD COLUMN additive text;');
  const target = r.commit();
  assert.throws(() => requirements(r.base, target, r.git), /Unclassified/);
  const to = r.git(['rev-parse', `HEAD:${file}`]).trim();
  const compatible = r.add('additive-schema', 'none', false, {
    kind: 'COMPATIBILITY',
    runningApplicationCompatible: true,
    retainedWorkCompatible: true,
    assets: { [file]: { from: null, to } },
    requiresDeployed: r.base,
  });
  assert.equal(requirements(r.base, compatible, r.git).sql.length, 1);
});
test('rollback cannot invoke the forward release path', (t) => {
  const r = repository(t);
  const target = r.add('later-release', 'none');
  assert.throws(() => requirements(target, r.base, r.git), /move forward/);
});

function liveFixture() {
  const resources = [
    { LogicalResourceId: 'Task', PhysicalResourceId: arn, ResourceType: 'AWS::DMS::ReplicationTask' },
    ...['source', 'target'].map((id) => ({
      LogicalResourceId: id,
      PhysicalResourceId: id,
      ResourceType: 'AWS::DMS::Endpoint',
    })),
    {
      LogicalResourceId: 'instance',
      PhysicalResourceId: 'instance',
      ResourceType: 'AWS::DMS::ReplicationInstance',
    },
  ];
  const live = Object.fromEntries(
    stacks.names(ctx).map((name) => [
      name,
      {
        stack: {
          StackId: name,
          LastUpdatedTime: '2026-10-01',
          Parameters: [
            { ParameterKey: 'CommitSHA', ParameterValue: baseline },
            { ParameterKey: 'CdcRouterEnabled', ParameterValue: 'true' },
            { ParameterKey: 'PeriodicMatcherEnabled', ParameterValue: 'false' },
          ],
        },
        resources: name === 'data' ? resources : [],
        template: { Resources: {} },
      },
    ]),
  );
  const api = (_ctx, _service, op, args) => {
    if (op === 'describe-replication-tasks') return { ReplicationTasks: [task()] };
    if (op === 'describe-endpoints')
      return { Endpoints: [{ EndpointArn: args[1].split('Values=')[1], Status: 'active' }] };
    assert.fail(`unexpected ${op}`);
  };
  const completed = {
    sha: baseline,
    stacks: stacks.snapshot(live),
    captureConfig: dms.captureConfig(ctx, task(), api),
  };
  const stored = { state: { ...empty(), completed }, etag: '"1"' };
  const targets = Object.fromEntries(Object.entries(live).map(([name, s]) => [name, s.template]));
  const digests = Object.fromEntries(
    loadCatalog().map((image) => [image.digestParameter, `sha256:${'1'.repeat(64)}`]),
  );
  return {
    live,
    stored,
    targets,
    digests,
    options: {
      api,
      protect: () => ({ protection_rules: [] }),
      readRequirements: () => ({ id: 'requirements', entries: [], manual: [] }),
    },
  };
}
test('plan rejects ambiguous adoption, stale baseline and missing activation values', () => {
  const f = liveFixture();
  const result = plan(ctx, f.stored, f.live, f.targets, f.digests, f.options);
  assert.equal(result.baseline, baseline);
  assert.throws(
    () => plan(ctx, { state: { ...empty(), completed: null } }, f.live, f.targets, f.digests, f.options),
    /baseline/,
  );
  f.live.compute.stack.Parameters[0].ParameterValue = sha;
  assert.throws(() => plan(ctx, f.stored, f.live, f.targets, f.digests, f.options), /differs/);
  f.live.compute.stack.Parameters = [];
  assert.throws(() => stacks.activation(f.live), /Missing\/invalid/);
});
for (const type of [
  'AWS::DMS::ReplicationTask',
  'AWS::DMS::Endpoint',
  'AWS::DMS::ReplicationInstance',
  'AWS::Kinesis::Stream',
  'AWS::RDS::DBInstance',
  'AWS::SQS::Queue',
])
  test(`routine plan rejects ${type} replacement/deletion or topology changes`, () => {
    const before = { Resources: { Owned: { Type: type, Properties: { Identity: 'old' } } } };
    const after = { Resources: { Owned: { Type: type, Properties: { Identity: 'new' } } } };
    assert.throws(() => stacks.guardChanges({ data: { template: before } }, { data: after }), /dedicated/);
  });
test('compatible DMS task settings may update only after the coordinator stops capture', () => {
  const before = {
    Resources: {
      Task: {
        Type: 'AWS::DMS::ReplicationTask',
        Properties: { ReplicationTaskSettings: 'old', TableMappings: 'same' },
      },
    },
  };
  const after = clone(before);
  after.Resources.Task.Properties.ReplicationTaskSettings = 'new';
  assert.ok(stacks.guardChanges({ data: { template: before } }, { data: after }).data.length);
  after.Resources.Task.Properties.TableMappings = 'retirement';
  assert.throws(() => stacks.guardChanges({ data: { template: before } }, { data: after }), /dedicated/);
});
test('routine updates cannot flip existing worker or scheduler activation', () => {
  for (const type of ['AWS::Lambda::EventSourceMapping', 'AWS::Scheduler::Schedule', 'AWS::Events::Rule']) {
    const before = { Resources: { Work: { Type: type, Properties: { Enabled: true } } } };
    const after = clone(before);
    after.Resources.Work.Properties.Enabled = false;
    assert.throws(
      () => stacks.guardChanges({ compute: { template: before } }, { compute: after }),
      /activation/,
    );
  }
});

test('a continuation after acquisition but before pause intent preserves its plan and can proceed', () => {
  const f = liveFixture();
  const approved = plan(ctx, f.stored, f.live, f.targets, f.digests, f.options);
  f.stored.state.active = {
    id: 'owned',
    kind: 'RELEASE',
    sha,
    phase: 'PREPARED',
    completed: [],
    activation: approved.activation,
    runtimeActivation: approved.runtimeActivation,
  };
  const next = plan(ctx, f.stored, f.live, f.targets, f.digests, f.options);
  assert.equal(next.continuation.phase, 'PREPARED');
  assert.equal(next.task.status, 'running');
});
function stackApi(status = 'UPDATE_COMPLETE') {
  return (_ctx, _service, operation, args) => {
    const name = args[1];
    if (operation === 'describe-stacks')
      return {
        Stacks: [
          {
            StackId: `arn:aws:cloudformation:${ctx.region}:${ctx.account}:stack/${name}/uuid`,
            StackStatus: status,
            CreationTime: '2026-10-01T00:00:00Z',
          },
        ],
      };
    if (operation === 'list-stack-resources')
      return {
        StackResourceSummaries: [
          {
            LogicalResourceId: 'Resource',
            PhysicalResourceId: 'physical',
            ResourceType: 'AWS::EC2::VPC',
            ResourceStatus: 'UPDATE_COMPLETE',
          },
        ],
      };
    if (operation === 'get-template')
      return { TemplateBody: { Resources: { Resource: { Type: 'AWS::EC2::VPC' } } } };
    assert.fail(operation);
  };
}
for (const status of [
  'CREATE_FAILED',
  'UPDATE_FAILED',
  'ROLLBACK_COMPLETE',
  'UPDATE_ROLLBACK_COMPLETE',
  'UPDATE_ROLLBACK_FAILED',
  'CREATE_IN_PROGRESS',
  'UPDATE_IN_PROGRESS',
  'REVIEW_IN_PROGRESS',
  'DELETE_IN_PROGRESS',
  'IMPORT_IN_PROGRESS',
  'IMPORT_ROLLBACK_IN_PROGRESS',
  'IMPORT_ROLLBACK_FAILED',
  'IMPORT_ROLLBACK_COMPLETE',
]) {
  test(`routine admission rejects ${status} before any mutation`, () => {
    assert.throws(() => stacks.inspect(ctx, stackApi(status)), /missing, unhealthy or updating/);
  });
}
test('healthy stacks without template parameters can be snapshotted deterministically', () => {
  const live = stacks.inspect(ctx, stackApi());
  assert.equal(Object.keys(stacks.snapshot(live)).length, 5);
});
test('capture configuration changes are rejected even when the task ARN remains stable', () => {
  const f = liveFixture();
  f.stored.state.completed.captureConfig = hash('different source slot');
  assert.throws(() => plan(ctx, f.stored, f.live, f.targets, f.digests, f.options), /endpoint\/slot/);
});

test('a completed target produces a verified idempotent plan instead of a second pause', () => {
  const f = liveFixture();
  f.stored.state.completed.sha = sha;
  for (const value of Object.values(f.live)) value.stack.Parameters[0].ParameterValue = sha;
  f.stored.state.completed.stacks = stacks.snapshot(f.live);
  const planned = plan(ctx, f.stored, f.live, f.targets, f.digests, f.options);
  assert.equal(planned.alreadyComplete, true);
});

'use strict';
const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const { spawnSync } = require('node:child_process');
const { hash } = require('./release/io.cjs');
const { loadCatalog } = require('./container-images.cjs');
const stacks = require('./release/stacks.cjs');
const { ctx, baseline } = require('./test-support/release.cjs');

function mockTool() {
  const fs = require('node:fs'),
    path = require('node:path'),
    assert = require('node:assert/strict');
  const tool = path.basename(process.argv[1]),
    args = process.argv.slice(2);
  const fixture = JSON.parse(fs.readFileSync('fixture.json'));
  const event = { tool, args };
  try {
    let result = {};
    if (tool === 'gh') {
      if (args[1].includes('/environments/')) result = fixture.environment;
      else {
        assert.ok(args[1].endsWith('/approvals'));
        result = [{ state: 'approved', user: { login: 'reviewer' }, environments: [{ id: 10 }] }];
      }
    } else if (tool === 'npm') {
      assert.deepEqual(args.slice(0, 6), ['--prefix', 'infra', 'run', 'cdk', '--', 'deploy']);
      assert.equal(args[args.indexOf('--app') + 1], path.resolve('.release/assembly'));
      assert.ok(fs.existsSync('state.json'), 'Durable ownership must precede deployment');
      const state = JSON.parse(fs.readFileSync('state.json'));
      assert.equal(state.active.id, process.env.RELEASE_ID);
      assert.equal(process.env.DEPLOY_SCOPE, fixture.scope);
      if (args[6].endsWith('-compute'))
        for (const [key, value] of Object.entries(fixture.digests))
          assert.ok(args.includes(`application-dev-compute:${key}=${value}`));
    } else {
      assert.equal(tool, 'aws');
      const arg = (key) => args[args.indexOf(key) + 1];
      if (args[0] === 'sts') result = { Account: fixture.account };
      else if (args[0] === 'cloudformation') {
        assert.equal(args[1], 'describe-stacks');
        result = { Stacks: [fixture.stack] };
      } else {
        assert.equal(args[0], 's3api');
        if (args[1] === 'get-object') {
          assert.ok(fs.existsSync('state.json'));
          const output = args[args.indexOf('--region') - 1];
          const state = JSON.parse(fs.readFileSync('state.json'));
          fs.writeFileSync(output, JSON.stringify(state));
          result = { ETag: `"${state.revision}"` };
        } else {
          assert.equal(args[1], 'put-object');
          const state = JSON.parse(fs.readFileSync('state.json'));
          assert.equal(arg('--if-match'), `"${state.revision}"`);
          const next = JSON.parse(fs.readFileSync(arg('--body')));
          fs.writeFileSync('state.json', JSON.stringify(next));
          result = { ETag: `"${next.revision}"` };
        }
      }
    }
    process.stdout.write(JSON.stringify(result));
  } catch (error) {
    event.unexpected = error.message;
    process.exitCode = 99;
  }
  fs.appendFileSync('commands.jsonl', JSON.stringify(event) + '\n');
}
function fixture(t, scope) {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'release-cli-'));
  t.after(() => fs.rmSync(dir, { recursive: true, force: true }));
  const write = (file, value) => {
    fs.mkdirSync(path.dirname(path.join(dir, file)), { recursive: true });
    fs.writeFileSync(path.join(dir, file), typeof value === 'string' ? value : JSON.stringify(value));
  };
  fs.symlinkSync(path.resolve(__dirname), path.join(dir, 'ci'));
  write('releases/protocol.json', { version: 1 });
  write('infra/scripts/ensure-dms-vpc-role.sh', '#!/bin/bash\nexit 0\n');
  write('bin/mock', `#!${process.execPath}\n(${mockTool.toString()})();\n`);
  fs.chmodSync(path.join(dir, 'bin/mock'), 0o755);
  for (const tool of ['aws', 'gh', 'npm']) fs.symlinkSync('mock', path.join(dir, `bin/${tool}`));
  write('commands.jsonl', '');
  const digests = Object.fromEntries(
    loadCatalog().map((image) => [image.digestParameter, `sha256:${'c'.repeat(64)}`]),
  );
  const stack = {
    StackStatus: 'UPDATE_COMPLETE',
    Parameters: [{ ParameterKey: 'CommitSHA', ParameterValue: baseline }],
  };
  const targets = Object.fromEntries(stacks.names(ctx).map((name) => [name, { Resources: {} }]));
  const plan = {
    version: 1,
    kind: scope,
    account: ctx.account,
    region: ctx.region,
    stage: ctx.stage,
    sha: ctx.sha,
    run: ctx.run,
    attempt: ctx.attempt,
    stateEtag: '"1"',
    inventory: Object.fromEntries(
      stacks.names(ctx).map((name) => [name, { sha: baseline, fingerprint: hash(stack) }]),
    ),
    artifacts: { digests, targets: stacks.templateHashes(targets) },
  };
  write('state.json', {
    version: 1,
    account: ctx.account,
    region: ctx.region,
    stage: ctx.stage,
    revision: 1,
    active: null,
    completed: null,
  });
  write('fixture.json', {
    account: ctx.account,
    scope,
    digests,
    stack,
    environment: {
      id: 10,
      name: 'aws-dev',
      can_admins_bypass: false,
      protection_rules: [{ type: 'required_reviewers', prevent_self_review: true, reviewers: [{ id: 7 }] }],
    },
  });
  write('.release/plan.json', plan);
  write('.release/digests.json', digests);
  for (const [name, template] of Object.entries(targets))
    write(`.release/assembly/application-dev-${name}.template.json`, template);
  const run = () =>
    spawnSync(process.execPath, ['ci/release.cjs', 'execute'], {
      cwd: dir,
      encoding: 'utf8',
      timeout: 20000,
      env: {
        PATH: `${path.join(dir, 'bin')}:${process.env.PATH}`,
        HOME: dir,
        STAGE: ctx.stage,
        DEPLOY_COMMIT_SHA: ctx.sha,
        AWS_ACCOUNT_ID: ctx.account,
        AWS_REGION: ctx.region,
        RELEASE_STATE_BUCKET: ctx.bucket,
        GITHUB_REPOSITORY: ctx.repository,
        GITHUB_RUN_ID: ctx.run,
        GITHUB_RUN_ATTEMPT: ctx.attempt,
        RELEASE_PLAN_ID: hash(plan),
        GITHUB_STEP_SUMMARY: path.join(dir, 'summary'),
      },
    });
  const events = () =>
    fs
      .readFileSync(path.join(dir, 'commands.jsonl'), 'utf8')
      .trim()
      .split('\n')
      .filter(Boolean)
      .map(JSON.parse);
  return { run, events, write, state: () => JSON.parse(fs.readFileSync(path.join(dir, 'state.json'))) };
}
for (const scope of ['foundation', 'all'])
  test(`real coordinator CLI carries pinned artifacts and ownership through exceptional ${scope}`, (t) => {
    const f = fixture(t, scope),
      result = f.run();
    assert.equal(result.error, undefined);
    assert.equal(result.status, 0, result.stderr);
    for (const event of f.events()) assert.equal(event.unexpected, undefined, event.unexpected);
    assert.deepEqual(
      f
        .events()
        .filter((e) => e.tool === 'npm')
        .map((e) => e.args[6]),
      ['network', 'data', 'initialize', ...(scope === 'all' ? ['compute', 'api'] : [])].map(
        (name) => `application-dev-${name}`,
      ),
    );
    assert.equal(f.state().active, null);
    assert.equal(f.state().adoptionRequired, true);
  });
test('a changed exceptional artifact is rejected before acquiring or advancing the stage', (t) => {
  const f = fixture(t, 'all');
  f.write('.release/digests.json', {});
  const result = f.run();
  assert.equal(result.status, 1);
  assert.match(result.stderr, /artifacts differ/);
  assert.equal(f.state().revision, 1);
  assert.ok(!f.events().some((e) => e.tool === 'npm'));
});

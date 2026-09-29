'use strict';

const test = require('node:test');
const assert = require('node:assert/strict');
const { chmodSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const { spawnSync } = require('node:child_process');
const { validateReleaseTag, resolveSource } = require('./release-source.cjs');

const helper = path.join(__dirname, 'release-source.cjs');
const sha = '6a9c07fa66163b0917434cbf6bcc20e40b9c845c';
const otherSha = 'b'.repeat(40);
const tag = '20260929-1234';
const tagRef = `refs/tags/${tag}`;
const develop = 'refs/remotes/origin/develop';
const formatError = 'Production requires a YYYYMMDD-HHMM release tag (UTC).';
const dateError = 'Release tag must contain a valid UTC date and time.';
const devRefError = 'Dev ref must be develop or a full lowercase commit SHA.';
const prodManual = { STAGE: 'prod', GITHUB_EVENT_NAME: 'workflow_dispatch', REQUESTED_REF: tag };
const prodPush = { STAGE: 'prod', GITHUB_EVENT_NAME: 'push', GITHUB_REF: tagRef, GITHUB_SHA: sha };
const devManual = { STAGE: 'dev', GITHUB_EVENT_NAME: 'workflow_dispatch' };
const devPush = { STAGE: 'dev', GITHUB_EVENT_NAME: 'push', GITHUB_REF: 'refs/heads/develop', GITHUB_SHA: sha };

function sourcePlan(ref, eventSha) {
  return [
    { args: ['rev-parse', '--verify', `${ref}^{commit}`], stdout: `${sha}\n` },
    ...(eventSha === undefined ? [] : [{ args: ['rev-parse', '--verify', `${eventSha}^{commit}`], stdout: `${sha}\n` }]),
    { args: ['merge-base', '--is-ancestor', sha, develop], stdout: '' },
  ];
}

function mockGit(t, plan) {
  const calls = [];
  t.after(() => assert.deepEqual(calls, plan.map((step) => step.args), 'Git calls must exactly match the plan'));
  return (args) => {
    const step = plan[calls.length];
    calls.push(args);
    assert.ok(step, `Unexpected Git call: ${JSON.stringify(args)}`);
    assert.deepEqual(args, step.args);
    if (step.error) throw step.error;
    return step.stdout;
  };
}

function rejectBeforeGit(t, env, message) {
  assert.throws(() => resolveSource(env, mockGit(t, [])), { message });
}

for (const value of ['20260101-0000', '20261231-2359', '20260430-1200', '20240229-2359', '20000229-0000', '24000229-1200', '19000228-1200']) {
  test(`CalVer accepts and preserves the exact UTC timestamp ${value}`, () => {
    assert.equal(validateReleaseTag(value), value);
  });
}

for (const value of [
  '', '20260929', '20260929-123', '20260929-12345', '2026929-1234', '2026-09-29-1234',
  '20260929_1234', '20260929-12:34', '20260929-1234Z', '20260929-123400',
  ' 20260929-1234', '20260929-1234 ', '20260929-1234\n', '20260929-1234\r',
  '20260929-1234\r\n', '20260929-1234\u2028', '20260929-1234\u2029',
  '\n20260929-1234', '20260929-12\n34', '２０２６０９２９-１２３４',
  `refs/tags/${tag}`, `${tag};echo injected`,
]) {
  test(`CalVer rejects non-exact format ${JSON.stringify(value)}`, () => {
    assert.throws(() => validateReleaseTag(value), { message: formatError });
  });
}

for (const value of [
  '20260001-1200', '20261301-1200', '20260100-1200', '20260132-1200',
  '20260431-1200', '20260230-1200', '20260229-1200', '19000229-1200', '21000229-1200',
  '20240230-1200', '20260929-2400', '20260929-2500', '20260929-1260', '20260929-1299',
]) {
  test(`CalVer rejects invalid calendar/time ${value}`, () => {
    assert.throws(() => validateReleaseTag(value), { message: dateError });
  });
}

const sources = [
  { name: 'manual prod existing tag', env: { ...prodManual, GITHUB_SHA: otherSha, GITHUB_REF: 'refs/heads/develop' }, ref: tagRef },
  { name: 'prod lightweight tag push', env: { ...prodPush, REQUESTED_REF: 'ignored' }, ref: tagRef, eventSha: sha },
  // The event may identify an annotated tag object; both refs must be peeled to commits.
  { name: 'prod annotated tag push', env: { ...prodPush, GITHUB_SHA: otherSha }, ref: tagRef, eventSha: otherSha },
  { name: 'manual dev default', env: { ...devManual, GITHUB_SHA: otherSha }, ref: develop },
  { name: 'manual dev blank default', env: { ...devManual, REQUESTED_REF: '' }, ref: develop },
  { name: 'manual dev explicit develop', env: { ...devManual, REQUESTED_REF: 'develop' }, ref: develop },
  { name: 'manual dev full SHA', env: { ...devManual, REQUESTED_REF: sha, GITHUB_SHA: otherSha }, ref: sha },
  { name: 'dev push event SHA, not the moving branch tip', env: { ...devPush, REQUESTED_REF: otherSha }, ref: sha, eventSha: sha },
];

for (const source of sources) {
  test(`${source.name} resolves once and pins the returned SHA/ref and Git arguments`, (t) => {
    const git = mockGit(t, sourcePlan(source.ref, source.eventSha));
    assert.deepEqual(resolveSource(source.env, git), { sha, ref: source.ref });
  });
}

for (const value of [undefined, '', 'staging', 'ephemeral', 'PROD', 'prod\n']) {
  test(`rejects unsupported stage ${JSON.stringify(value)} before Git`, (t) => {
    rejectBeforeGit(t, { ...prodManual, STAGE: value }, 'Stage must be dev or prod.');
  });
}

for (const stage of ['dev', 'prod']) {
  for (const event of [undefined, '', 'pull_request', 'workflow_call', 'workflow_run', 'schedule', 'push\n']) {
    test(`${stage} rejects unsupported event ${JSON.stringify(event)} before Git`, (t) => {
      rejectBeforeGit(t, { ...prodManual, STAGE: stage, GITHUB_EVENT_NAME: event }, 'Unsupported deployment event.');
    });
  }
}

for (const value of [undefined, '', 'develop', 'prod', sha, tagRef]) {
  test(`manual prod requires a bare CalVer tag, not ${JSON.stringify(value)}`, (t) => {
    rejectBeforeGit(t, { ...prodManual, REQUESTED_REF: value }, formatError);
  });
}

for (const env of [prodManual, prodPush]) {
  test(`prod ${env.GITHUB_EVENT_NAME} rejects an invalid calendar date before Git`, (t) => {
    rejectBeforeGit(t, { ...env, REQUESTED_REF: '20260230-1200', GITHUB_REF: 'refs/tags/20260230-1200' }, dateError);
  });
}

for (const value of [undefined, '', 'refs/heads/develop', `refs/heads/${tag}`, `refs/remotes/origin/${tag}`]) {
  test(`prod push rejects non-tag ref ${JSON.stringify(value)}`, (t) => {
    rejectBeforeGit(t, { ...prodPush, GITHUB_REF: value }, formatError);
  });
}

test('a bare CalVer string is not a production push tag ref', (t) => {
  rejectBeforeGit(t, { ...prodPush, GITHUB_REF: tag }, 'Production pushes must select a release tag.');
});

for (const value of ['refs/heads/main', 'refs/heads/prod', 'develop', `refs/tags/${tag}`, undefined, 'refs/heads/develop\n']) {
  test(`dev push rejects unsupported ref ${JSON.stringify(value)}`, (t) => {
    rejectBeforeGit(t, { ...devPush, GITHUB_REF: value }, 'Dev pushes must come from develop.');
  });
}

for (const value of ['main', 'prod', 'refs/heads/develop', develop, tag, 'HEAD', 'HEAD~1', sha.slice(0, 7), sha.toUpperCase(), `${sha}0`, `${sha}\n`, `${sha}\r`]) {
  test(`manual dev rejects non-exact develop/SHA ${JSON.stringify(value)}`, (t) => {
    rejectBeforeGit(t, { ...devManual, REQUESTED_REF: value }, devRefError);
  });
}

for (const value of [undefined, '', 'develop', 'HEAD', sha.slice(0, 39), `${sha}0`, sha.toUpperCase(), 'g'.repeat(40), `${sha}\n`, `${sha}\r`]) {
  test(`dev push rejects invalid event SHA ${JSON.stringify(value)}`, (t) => {
    rejectBeforeGit(t, { ...devPush, GITHUB_SHA: value }, 'Invalid push commit SHA.');
  });
}

for (const value of ['develop;echo injected', '$(echo injected)', '`echo injected`', 'develop && echo injected', 'develop|cat', 'develop>output', 'develop\necho injected', "develop'", 'develop"', '--help', '*', `${tag};echo injected`]) {
  for (const stage of ['dev', 'prod']) {
    test(`${stage} rejects shell metacharacter/option ref ${JSON.stringify(value)} before Git`, (t) => {
      rejectBeforeGit(t, { ...devManual, STAGE: stage, REQUESTED_REF: value }, stage === 'prod' ? formatError : devRefError);
    });
  }
}

for (const env of [prodManual, prodPush]) {
  test(`prod ${env.GITHUB_EVENT_NAME} requires the existing namespaced tag, without branch/SHA fallback`, (t) => {
    const error = Object.assign(new Error('fatal: Needed a single revision'), { status: 128 });
    const git = mockGit(t, [{ args: ['rev-parse', '--verify', `${tagRef}^{commit}`], error }]);
    assert.throws(() => resolveSource(env, git), (actual) => actual === error);
  });
}

for (const eventSha of [sha, otherSha]) {
  test(`prod push rejects a moved tag after peeling event ${eventSha}`, (t) => {
    const plan = sourcePlan(tagRef, eventSha).slice(0, 2);
    plan[0].stdout = `${otherSha}\n`;
    assert.throws(() => resolveSource({ ...prodPush, GITHUB_SHA: eventSha }, mockGit(t, plan)), {
      message: 'Release tag no longer matches the triggering commit.',
    });
  });
}

test('a missing triggering tag object fails closed instead of skipping the event SHA guard', (t) => {
  const error = Object.assign(new Error('fatal: triggering object not found'), { status: 128 });
  const plan = sourcePlan(tagRef, otherSha).slice(0, 2);
  plan[1].error = error;
  assert.throws(() => resolveSource({ ...prodPush, GITHUB_SHA: otherSha }, mockGit(t, plan)), (actual) => actual === error);
});

for (const source of sources) {
  for (const status of [1, 128]) {
    test(`${source.name} propagates develop ancestry failure ${status}`, (t) => {
      const error = Object.assign(new Error(status === 1 ? 'commit is not on develop' : 'fatal: origin/develop is unavailable'), { status });
      const plan = sourcePlan(source.ref, source.eventSha);
      plan.at(-1).error = error;
      assert.throws(() => resolveSource(source.env, mockGit(t, plan)), (actual) => actual === error);
    });
  }
}

for (const stdout of ['', '\n', sha.slice(0, 39), sha.toUpperCase(), 'g'.repeat(40), `${sha}\n${otherSha}\n`, 'not a commit\n']) {
  test(`rejects malformed resolved commit ${JSON.stringify(stdout)}`, (t) => {
    const git = mockGit(t, [{ args: ['rev-parse', '--verify', `${develop}^{commit}`], stdout }]);
    assert.throws(() => resolveSource(devManual, git), { message: 'Could not resolve the deployment commit.' });
  });
}

// The CLI uses this executable rather than any installed Git. Unexpected calls
// are logged separately so an intentional failure cannot hide a broken mock.
function fakeGit() {
  const assert = require('node:assert/strict');
  const fs = require('node:fs');
  const plan = JSON.parse(fs.readFileSync(process.env.TEST_GIT_PLAN, 'utf8'));
  const previous = fs.readFileSync(process.env.TEST_GIT_LOG, 'utf8').trim();
  const record = { args: process.argv.slice(2) };
  try {
    const step = plan[previous ? previous.split('\n').length : 0];
    assert.ok(step, 'Unexpected extra Git call');
    assert.deepEqual(record.args, ['--no-pager', ...step.args]);
    fs.appendFileSync(process.env.TEST_GIT_LOG, JSON.stringify(record) + '\n');
    process.stdout.write(step.stdout ?? '');
    process.stderr.write(step.stderr ?? '');
    process.exitCode = step.exitCode ?? 0;
  } catch (error) {
    record.unexpected = error.message;
    fs.appendFileSync(process.env.TEST_GIT_LOG, JSON.stringify(record) + '\n');
    process.exitCode = 99;
  }
}

function runCli(t, env, plan) {
  const directory = mkdtempSync(path.join(os.tmpdir(), 'release-source-test-'));
  t.after(() => rmSync(directory, { recursive: true, force: true }));
  const bin = path.join(directory, 'bin');
  mkdirSync(bin);
  const git = path.join(bin, 'git');
  writeFileSync(git, `#!${process.execPath}\n(${fakeGit.toString()})();\n`);
  chmodSync(git, 0o755);
  const planFile = path.join(directory, 'plan.json');
  const logFile = path.join(directory, 'git.jsonl');
  const outputFile = path.join(directory, 'output');
  const summaryFile = path.join(directory, 'summary');
  writeFileSync(planFile, JSON.stringify(plan));
  writeFileSync(logFile, '');
  writeFileSync(outputFile, 'previous=value\n');
  writeFileSync(summaryFile, '# Existing summary\n');
  const result = spawnSync(process.execPath, [helper], {
    cwd: directory,
    // Only the fake Git is executable via PATH; no inherited credentials or Git config.
    env: { PATH: bin, HOME: directory, TEST_GIT_PLAN: planFile, TEST_GIT_LOG: logFile, GITHUB_OUTPUT: outputFile, GITHUB_STEP_SUMMARY: summaryFile, ...env },
    encoding: 'utf8',
    timeout: 15000,
  });
  assert.equal(result.error, undefined);
  assert.equal(result.signal, null);
  const calls = readFileSync(logFile, 'utf8').trim().split('\n').filter(Boolean).map(JSON.parse);
  for (const call of calls) assert.equal(call.unexpected, undefined, call.unexpected);
  assert.deepEqual(calls.map((call) => call.args), plan.map((step) => ['--no-pager', ...step.args]));
  return { ...result, output: readFileSync(outputFile, 'utf8'), summary: readFileSync(summaryFile, 'utf8') };
}

for (const source of sources) {
  test(`CLI ${source.name} appends only the pinned output and summary`, (t) => {
    const result = runCli(t, source.env, sourcePlan(source.ref, source.eventSha));
    assert.equal(result.status, 0, result.stderr);
    assert.equal(result.stdout, '');
    assert.equal(result.stderr, '');
    assert.equal(result.output, `previous=value\ncommit_sha=${sha}\n`);
    assert.equal(result.summary, `# Existing summary\n## Deployment source\n- Stage: ${source.env.STAGE}\n- Ref: ${source.ref}\n- Commit: ${sha}\n`);
  });
}

for (const failure of ['validation', 'missing tag', 'moved tag', 'ancestry']) {
  test(`CLI ${failure} exits nonzero without emitting a deployment output/summary`, (t) => {
    const env = failure === 'validation' ? { ...prodManual, REQUESTED_REF: '' } : prodPush;
    let plan = sourcePlan(tagRef, sha);
    if (failure === 'validation') plan = [];
    if (failure === 'missing tag') plan = [{ ...plan[0], stdout: '', stderr: 'fatal: tag does not exist\n', exitCode: 128 }];
    if (failure === 'moved tag') plan = [{ ...plan[0], stdout: `${otherSha}\n` }, plan[1]];
    if (failure === 'ancestry') plan.at(-1).exitCode = 1;
    const result = runCli(t, env, plan);
    assert.equal(result.status, 1);
    assert.equal(result.stdout, '');
    assert.match(result.stderr, /^Release validation failed: /);
    assert.equal(result.output, 'previous=value\n');
    assert.equal(result.summary, '# Existing summary\n');
  });
}

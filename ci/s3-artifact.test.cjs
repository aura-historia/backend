'use strict';

const test = require('node:test');
const assert = require('node:assert/strict');
const { chmodSync, copyFileSync, mkdirSync, mkdtempSync, readFileSync, readdirSync, rmSync, writeFileSync } = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const { spawnSync } = require('node:child_process');

const root = path.resolve(__dirname, '..');
const helper = path.join(__dirname, 's3-artifact-exists.sh');
const sha = '6a9c07fa66163b0917434cbf6bcc20e40b9c845c';
const bucket = 'test-binary-artifacts';
const binary = 'aura-historia-api';
const existing = { stdout: JSON.stringify({ AcceptRanges: 'bytes', ContentLength: 123, ETag: '"test-etag"', Metadata: {} }) + '\n', exitCode: 0 };
const missing = { stderr: 'An error occurred (404) when calling the HeadObject operation: Not Found\n', exitCode: 254 };
const failures = [
  { name: '403 forbidden', stderr: 'An error occurred (403) when calling the HeadObject operation: Forbidden\n', exitCode: 254 },
  { name: 'access denied', stderr: 'An error occurred (AccessDenied) when calling the HeadObject operation: Access Denied\n', exitCode: 254 },
  { name: 'network failure', stderr: 'Could not connect to the endpoint URL: "https://s3.invalid/"\n', exitCode: 255 },
  { name: 'timeout', stderr: 'Read timeout on endpoint URL: "https://s3.invalid/"\n', exitCode: 255 },
  { name: 'missing credentials', stderr: 'Unable to locate credentials.\n', exitCode: 253 },
  { name: 'server error', stderr: 'An error occurred (500) when calling the HeadObject operation: Internal Server Error\n', exitCode: 254 },
  { name: 'malformed error', stderr: '{"Error":\n', exitCode: 254 },
  { name: 'bare status is not an explicit HeadObject error', stderr: '404\n', exitCode: 254 },
  { name: 'silent nonzero exit', stderr: '', exitCode: 42 },
  { name: 'success-looking stdout with a failed command', stdout: existing.stdout, stderr: 'response truncated\n', exitCode: 2 },
  { name: '404 on stdout, not the AWS error channel', stdout: missing.stderr, stderr: 'invalid response\n', exitCode: 254 },
];
const misleadingErrors = [
  { name: 'malformed error containing a parenthesized 404', stderr: 'Proxy response could not be parsed (404)\n', exitCode: 255 },
  { name: 'AccessDenied message containing a 404 key', stderr: 'An error occurred (AccessDenied) when calling the HeadObject operation: denied key archive/(404).zip\n', exitCode: 254 },
  { name: 'network error containing a NotFound key', stderr: 'Could not connect to the endpoint URL: "https://s3.invalid/archive/(NotFound).zip"\n', exitCode: 255 },
  { name: 'non-HeadObject NoSuchKey error', stderr: 'An error occurred (NoSuchKey) when calling the GetObject operation: wrong operation\n', exitCode: 254 },
];

// There is no AWS fallback. Check every argument and reject extra calls, then
// check the separate unexpected-call log even when the helper is meant to fail.
function fakeAws() {
  const assert = require('node:assert/strict');
  const fs = require('node:fs');
  const plan = JSON.parse(fs.readFileSync(process.env.TEST_AWS_PLAN, 'utf8'));
  const previous = fs.readFileSync(process.env.TEST_AWS_LOG, 'utf8').trim();
  const record = { args: process.argv.slice(2) };
  try {
    assert.equal(previous, '', 'Unexpected extra AWS call');
    assert.deepEqual(record.args, plan.args, 'Unexpected AWS command or arguments');
    fs.appendFileSync(process.env.TEST_AWS_LOG, JSON.stringify(record) + '\n');
    process.stdout.write(plan.stdout ?? '');
    process.stderr.write(plan.stderr ?? '');
    process.exitCode = plan.exitCode;
  } catch (error) {
    record.unexpected = error.message;
    fs.appendFileSync(process.env.TEST_AWS_LOG, JSON.stringify(record) + '\n');
    process.exitCode = 99;
  }
}

function lambdaCheckScript() {
  const lines = readFileSync(path.join(root, '.github/workflows/deploy.yml'), 'utf8').split('\n');
  const job = lines.indexOf('  aws-push-lambda:');
  assert.notEqual(job, -1, 'Missing Lambda publishing job');
  const nextJob = lines.findIndex((line, index) => index > job && /^  [a-z][\w-]*:/.test(line));
  const start = lines.indexOf('      - name: Check for published ZIP', job);
  assert.ok(start > job && (nextJob === -1 || start < nextJob), 'Missing Lambda existence check');
  const run = lines.indexOf('        run: |', start);
  const nextStep = lines.findIndex((line, index) => index > start && line.startsWith('      - '));
  assert.ok(run > start && (nextStep === -1 || run < nextStep), 'Missing existence-check script');
  const body = [];
  for (let index = run + 1; index < lines.length && lines[index].startsWith('          '); index += 1) {
    body.push(lines[index].slice(10));
  }
  return body.join('\n');
}

function runCheck(t, response, options = {}) {
  const selectedBucket = options.bucket ?? bucket;
  const selectedBinary = options.binary ?? binary;
  const selectedStage = options.stage ?? 'dev';
  const selectedKey = options.key ?? `${selectedBinary}-${selectedStage}-${sha}.zip`;
  const directory = mkdtempSync(path.join(os.tmpdir(), 's3-artifact-test-'));
  t.after(() => rmSync(directory, { recursive: true, force: true }));
  const bin = path.join(directory, 'bin');
  const temporary = path.join(directory, 'tmp');
  mkdirSync(bin);
  mkdirSync(temporary);
  mkdirSync(path.join(directory, 'ci'));
  copyFileSync(helper, path.join(directory, 'ci/s3-artifact-exists.sh'));
  const aws = path.join(bin, 'aws');
  writeFileSync(aws, `#!${process.execPath}\n(${fakeAws.toString()})();\n`);
  chmodSync(aws, 0o755);
  const planFile = path.join(directory, 'plan.json');
  const logFile = path.join(directory, 'aws.jsonl');
  const outputFile = path.join(directory, 'output');
  const args = ['s3api', 'head-object', '--bucket', selectedBucket, '--key', selectedKey];
  writeFileSync(planFile, JSON.stringify({ ...response, args }));
  writeFileSync(logFile, '');
  writeFileSync(outputFile, 'previous=value\n');
  // Match the runner's default bash -e behavior, without depending on pipefail
  // or rewriting the real workflow snippet to make it safer than production.
  const command = options.workflow
    ? ['--noprofile', '--norc', '-e', '-c', lambdaCheckScript()]
    : ['--noprofile', '--norc', helper, selectedBucket, selectedKey];
  const result = spawnSync('bash', command, {
    cwd: directory,
    // No inherited AWS credentials/configuration or shell startup hooks.
    env: {
      PATH: `${bin}${path.delimiter}${process.env.PATH}`,
      HOME: directory,
      TMPDIR: temporary,
      AWS_EC2_METADATA_DISABLED: 'true',
      AWS_CONFIG_FILE: path.join(directory, 'unused-config'),
      AWS_SHARED_CREDENTIALS_FILE: path.join(directory, 'unused-credentials'),
      TEST_AWS_PLAN: planFile,
      TEST_AWS_LOG: logFile,
      GITHUB_OUTPUT: outputFile,
      BUCKET: selectedBucket,
      BINARY: selectedBinary,
      STAGE: selectedStage,
      DEPLOY_COMMIT_SHA: sha,
    },
    encoding: 'utf8',
    timeout: 15000,
  });
  assert.equal(result.error, undefined);
  assert.equal(result.signal, null);
  const calls = readFileSync(logFile, 'utf8').trim().split('\n').filter(Boolean).map(JSON.parse);
  for (const call of calls) assert.equal(call.unexpected, undefined, call.unexpected);
  assert.deepEqual(calls.map((call) => call.args), [args], 'Expected exactly one read-only HeadObject call');
  assert.deepEqual(readdirSync(temporary), [], 'Temporary AWS error files must be removed on every exit');
  return { ...result, output: readFileSync(outputFile, 'utf8') };
}

test('a valid existing S3 object emits exactly true, not the HeadObject payload', (t) => {
  const result = runCheck(t, existing);
  assert.equal(result.status, 0, result.stderr);
  assert.equal(result.stdout, 'true\n');
  assert.equal(result.stderr, '');
  assert.equal(result.output, 'previous=value\n');
});

for (const code of ['404', 'NoSuchKey', 'NotFound']) {
  test(`an explicit HeadObject ${code} is genuine absence and emits exactly false`, (t) => {
    const result = runCheck(t, { ...missing, stderr: `An error occurred (${code}) when calling the HeadObject operation: Not Found\n` });
    assert.equal(result.status, 0, result.stderr);
    assert.equal(result.stdout, 'false\n');
    assert.equal(result.stderr, '');
    assert.equal(result.output, 'previous=value\n');
  });
}

for (const response of [...failures, ...misleadingErrors]) {
  test(`S3 ${response.name} fails closed without a boolean output`, (t) => {
    const result = runCheck(t, response);
    assert.equal(result.status, 1, 'A failed lookup must not be converted into absence');
    assert.equal(result.stdout, '');
    assert.equal(result.stderr, response.stderr);
    assert.equal(result.output, 'previous=value\n');
  });
}

test('bucket and key shell metacharacters, whitespace and globs remain literal AWS arguments', (t) => {
  const result = runCheck(t, existing, {
    bucket: 'bucket space;$(printf injected)`printf injected`&*',
    key: 'directory/"quoted" \'key\';$(printf injected)`printf injected`|&<>?*\n--endpoint-url=https://invalid',
  });
  assert.equal(result.status, 0, result.stderr);
  assert.equal(result.stdout, 'true\n');
  assert.equal(result.stderr, '');
});

test('Lambda workflow assigns the helper result before echo, rather than swallowing command-substitution failure', () => {
  assert.equal(lambdaCheckScript(), [
    'exists="$(bash ci/s3-artifact-exists.sh "$BUCKET" "${BINARY}-${STAGE}-${DEPLOY_COMMIT_SHA}.zip")"',
    'echo "exists=$exists" >> "$GITHUB_OUTPUT"',
  ].join('\n'));
});

for (const [name, response, expected] of [['existing', existing, 'true'], ['absent', missing, 'false']]) {
  test(`real Lambda workflow ${name} lookup appends the exact exists output`, (t) => {
    const result = runCheck(t, response, { workflow: true });
    assert.equal(result.status, 0, result.stderr);
    assert.equal(result.stdout, '');
    assert.equal(result.stderr, '');
    assert.equal(result.output, `previous=value\nexists=${expected}\n`);
  });
}

for (const response of failures) {
  test(`real Lambda workflow preserves ${response.name} failure before writing GITHUB_OUTPUT`, (t) => {
    const result = runCheck(t, response, { workflow: true });
    assert.equal(result.status, 1);
    assert.equal(result.stdout, '');
    assert.equal(result.stderr, response.stderr);
    assert.equal(result.output, 'previous=value\n', 'The echo must not run after a failed assignment');
  });
}

test('real Lambda workflow passes bucket, binary, stage and pinned SHA through as literals', (t) => {
  const result = runCheck(t, existing, {
    workflow: true,
    bucket: 'bucket $(printf injected);*',
    binary: 'binary `printf injected`&"\'\n*',
    stage: 'dev;$(printf injected)',
  });
  assert.equal(result.status, 0, result.stderr);
  assert.equal(result.stdout, '');
  assert.equal(result.stderr, '');
  assert.equal(result.output, 'previous=value\nexists=true\n');
});

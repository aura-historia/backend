'use strict';

const test = require('node:test');
const assert = require('node:assert/strict');
const { chmodSync, mkdirSync, mkdtempSync, readFileSync, readdirSync, rmSync, writeFileSync } = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const { spawnSync } = require('node:child_process');

const root = path.resolve(__dirname, '..');
const helper = path.join(root, 'ci/invoke-operation.sh');
const sha = '6a9c07fa66163b0917434cbf6bcc20e40b9c845c';
const otherSha = 'b'.repeat(40);
const privateContent = 'PRIVATE_PROVIDER_CONTENT_MUST_NOT_BE_LOGGED';

// This executable has no AWS fallback: every command, argument and ordering must
// match the fixture, even in tests that expect the helper to fail.
function fakeAws() {
  const assert = require('node:assert/strict');
  const fs = require('node:fs');
  const path = require('node:path');
  const plan = JSON.parse(fs.readFileSync(process.env.TEST_AWS_PLAN, 'utf8'));
  const previous = fs.readFileSync(process.env.TEST_AWS_LOG, 'utf8').trim();
  const index = previous ? previous.split('\n').length : 0;
  const record = { args: process.argv.slice(2) };
  try {
    const step = plan[index];
    assert.ok(step, 'Unexpected extra AWS call');
    assert.equal(process.env.AWS_PAGER, '');
    assert.equal(process.env.AWS_CLI_AUTO_PROMPT, 'off');
    const normalized = [...record.args];
    let resultFile;
    if (step.args.includes('@payload')) {
      const payloadIndex = step.args.indexOf('@payload');
      assert.ok(normalized[payloadIndex].startsWith('fileb://'));
      const payloadFile = normalized[payloadIndex].slice('fileb://'.length);
      assert.ok(payloadFile.startsWith(process.env.TMPDIR + path.sep));
      record.payload = JSON.parse(fs.readFileSync(payloadFile, 'utf8'));
      normalized[payloadIndex] = '@payload';
      const resultIndex = step.args.indexOf('@result');
      resultFile = normalized[resultIndex];
      assert.ok(resultFile.startsWith(process.env.TMPDIR + path.sep));
      normalized[resultIndex] = '@result';
    }
    assert.deepEqual(normalized, step.args, 'Unexpected AWS command or arguments');
    if (Object.hasOwn(step, 'result')) fs.writeFileSync(resultFile, step.result);
    fs.appendFileSync(process.env.TEST_AWS_LOG, JSON.stringify(record) + '\n');
    process.stdout.write(step.stdout ?? '');
    process.stderr.write(step.stderr ?? '');
    process.exitCode = step.exitCode ?? 0;
  } catch (error) {
    record.unexpected = error.message;
    fs.appendFileSync(process.env.TEST_AWS_LOG, JSON.stringify(record) + '\n');
    process.exitCode = 99;
  }
}

function deployedStack(stage = 'dev', commit = sha, status = 'UPDATE_COMPLETE') {
  return {
    Stacks: [{
      StackName: `application-${stage}-initialize`,
      StackStatus: status,
      Parameters: [{ ParameterKey: 'CommitSHA', ParameterValue: commit }],
    }],
  };
}

function callPlan({ operation = 'migrate', stage = 'dev', commit = sha, region = 'eu-central-1', status = 'UPDATE_COMPLETE' } = {}) {
  const functionName = `${operation === 'migrate' ? 'database-migration' : 'fxrate'}-lambda-${stage}`;
  return [
    {
      args: ['cloudformation', 'describe-stacks', '--stack-name', `application-${stage}-initialize`, '--region', region, '--output', 'json'],
      stdout: JSON.stringify(deployedStack(stage, commit, status)),
    },
    {
      args: ['lambda', 'wait', 'function-updated', '--function-name', functionName, '--region', region, '--cli-connect-timeout', '10', '--cli-read-timeout', '30'],
    },
    {
      args: ['lambda', 'invoke', '--function-name', functionName, '--region', region, '--invocation-type', 'RequestResponse', '--payload', '@payload', '--cli-binary-format', 'raw-in-base64-out', '--cli-connect-timeout', '10', '--cli-read-timeout', '900', '--output', 'json', '@result'],
      stdout: '{"StatusCode":200,"ExecutedVersion":"$LATEST"}',
      result: operation === 'migrate' ? '{"status":"ready"}' : 'null',
    },
  ];
}

function runOperation(t, options = {}) {
  const { operation = 'migrate', stage = 'dev', commit = sha, region = 'eu-central-1', args = [operation], env = {} } = options;
  const plan = options.plan ?? callPlan({ operation, stage, commit, region });
  const directory = mkdtempSync(path.join(os.tmpdir(), 'operations-test-'));
  t.after(() => rmSync(directory, { recursive: true, force: true }));
  const bin = path.join(directory, 'bin');
  const temporary = path.join(directory, 'tmp');
  mkdirSync(bin);
  mkdirSync(temporary);
  const aws = path.join(bin, 'aws');
  writeFileSync(aws, `#!/usr/bin/env node\n(${fakeAws.toString()})();\n`);
  chmodSync(aws, 0o755);
  const planFile = path.join(directory, 'plan.json');
  const logFile = path.join(directory, 'aws.jsonl');
  const summaryFile = path.join(directory, 'summary.md');
  writeFileSync(planFile, JSON.stringify(plan));
  writeFileSync(logFile, '');
  writeFileSync(summaryFile, '');
  // No repository checkout, AWS credentials, or inherited AWS configuration.
  const result = spawnSync('bash', [helper, ...args], {
    cwd: directory,
    env: {
      PATH: `${bin}${path.delimiter}${process.env.PATH}`,
      HOME: directory,
      TMPDIR: temporary,
      STAGE: stage,
      DEPLOY_COMMIT_SHA: commit,
      AWS_REGION: region,
      GITHUB_SHA: otherSha,
      GITHUB_STEP_SUMMARY: summaryFile,
      TEST_AWS_PLAN: planFile,
      TEST_AWS_LOG: logFile,
      ...env,
    },
    encoding: 'utf8',
    timeout: 15000,
  });
  assert.equal(result.error, undefined);
  const log = readFileSync(logFile, 'utf8').trim();
  const calls = log ? log.split('\n').map(JSON.parse) : [];
  for (const call of calls) assert.equal(call.unexpected, undefined, call.unexpected);
  assert.equal(calls.length, plan.length, 'Expected exactly the planned AWS calls');
  const summary = readFileSync(summaryFile, 'utf8');
  assert.ok(!`${result.stdout}${result.stderr}${summary}`.includes(privateContent), 'Provider content leaked');
  assert.deepEqual(readdirSync(temporary), [], 'Temporary provider responses must be removed');
  if (result.status !== 0) assert.equal(summary, '', 'Do not summarize a failed operation as successful');
  return { ...result, calls, summary };
}

function assertSuccess(result, operation, stage, commit = sha) {
  assert.equal(result.status, 0, result.stderr);
  assert.equal(result.summary, `- Operation: ${operation}\n- Stage: ${stage}\n- Deployed initialization SHA: ${commit}\n`);
}

for (const operation of ['migrate', 'initialize']) {
  test(`${operation} workflow is manual, protected, serialized and invocation-only`, () => {
    const workflow = readFileSync(path.join(root, `.github/workflows/${operation}.yml`), 'utf8');
    const name = operation === 'migrate' ? 'Migrate' : 'Initialize';
    assert.ok(workflow.startsWith(`name: ${name} (CD)\n`));
    assert.match(workflow, /on:\n  workflow_dispatch:\n    inputs:\n/);
    assert.doesNotMatch(workflow, /^  (?:push|pull_request|schedule|workflow_call|workflow_run):/m);
    assert.match(workflow, /stage:\n        description: .+\n        required: true\n        type: choice\n        options:\n          - dev\n          - prod\n/);
    assert.match(workflow, /commit_sha:\n        description: .*Expected DEPLOYED initialization stack CommitSHA.*full 40-character lowercase SHA.*not a source checkout.*\n        required: true\n        type: string/);
    assert.ok(workflow.includes('concurrency:\n  group: aws-deploy-${{ inputs.stage }}\n  cancel-in-progress: false'));
    assert.ok(workflow.includes('environment:\n      name: aws-${{ inputs.stage }}'));
    assert.match(workflow, /timeout-minutes: 20/);
    assert.match(workflow, /permissions:\n      contents: read\n      id-token: write/);
    assert.ok(workflow.includes('STAGE: ${{ inputs.stage }}'));
    assert.ok(workflow.includes('DEPLOY_COMMIT_SHA: ${{ inputs.commit_sha }}'));
    assert.ok(workflow.includes('AWS_REGION: ${{ vars.AWS_REGION }}'));
    assert.match(workflow, /uses: actions\/checkout@v7\n        with:\n          ref: \$\{\{ github\.sha \}\}/);
    assert.match(workflow, /uses: aws-actions\/configure-aws-credentials@v6\.3\.0\n        with:\n          role-to-assume: \$\{\{ secrets\.CI_DEPLOY_ROLE_ARN \}\}\n          aws-region: \$\{\{ vars\.AWS_REGION \}\}/);
    assert.deepEqual([...workflow.matchAll(/\buses: (.+)/g)].map((match) => match[1]), [
      'actions/checkout@v7', 'actions/setup-node@v7', 'aws-actions/configure-aws-credentials@v6.3.0',
    ]);
    assert.deepEqual([...workflow.matchAll(/^        run: (.+)$/gm)].map((match) => match[1]), [`bash ci/invoke-operation.sh ${operation}`]);
    assert.doesNotMatch(workflow, /npm|\bcdk\b|resolve-container-images|git rev-parse|fetch-depth/);
    if (operation === 'initialize') {
      assert.match(workflow, /First-time FX bootstrap only, after Migrate; retries reuse the stable event ID \(no deployment\)/);
      assert.doesNotMatch(workflow, /database-migration|invoke-operation\.sh migrate/);
    }
    const deploy = readFileSync(path.join(root, '.github/workflows/deploy.yml'), 'utf8');
    assert.match(deploy, /concurrency:\n  group: aws-deploy-[^\n]+\n  cancel-in-progress: false/);
  });
}

for (const stage of ['dev', 'prod']) {
  for (const status of ['CREATE_COMPLETE', 'UPDATE_COMPLETE', 'UPDATE_ROLLBACK_COMPLETE']) {
    test(`migrate invokes only the deployed migration Lambda: ${stage}, ${status}`, (t) => {
      const plan = callPlan({ stage, status });
      // Metadata may contain a null FunctionError; neither it nor provider stderr is logged.
      plan[2].stdout = JSON.stringify({ StatusCode: 200, FunctionError: null, LogResult: privateContent });
      plan[2].stderr = privateContent;
      plan[2].result = ' \n { "status" : "ready" } \n';
      const result = runOperation(t, { stage, plan });
      assertSuccess(result, 'migrate', stage);
      assert.deepEqual(result.calls[2].payload, {});
    });
  }

  test(`initialize captures FX with one stable ${stage} ID across retries and releases`, (t) => {
    const payloads = [];
    for (const commit of [sha, sha, otherSha]) {
      const result = runOperation(t, { operation: 'initialize', stage, commit, region: 'us-east-1' });
      assertSuccess(result, 'initialize', stage, commit);
      payloads.push(result.calls[2].payload);
    }
    const expected = {
      version: '0',
      id: `deployment:fxrate:initial:${stage}:v1`,
      'detail-type': 'Scheduled Event',
      source: 'aura-historia.deployment',
      account: '000000000000',
      time: '1970-01-01T00:00:00Z',
      region: 'us-east-1',
      resources: [],
      detail: {},
    };
    for (const payload of payloads) assert.deepEqual(payload, expected);
  });
}

for (const invalid of [undefined, '', 'a'.repeat(39), 'a'.repeat(41), 'A'.repeat(40), 'g'.repeat(40), `${sha}\n`, `$(echo ${privateContent})`]) {
  test(`reject invalid input SHA ${JSON.stringify(invalid)} before AWS`, (t) => {
    const result = runOperation(t, { env: { DEPLOY_COMMIT_SHA: invalid }, plan: [] });
    assert.equal(result.status, 1);
    assert.match(result.stderr, /full 40-character lowercase Git commit SHA/);
  });
}

for (const invalid of [undefined, '', 'staging', 'DEV', `dev\n${privateContent}`]) {
  test(`reject invalid stage ${JSON.stringify(invalid)} before AWS`, (t) => {
    const result = runOperation(t, { env: { STAGE: invalid }, plan: [] });
    assert.equal(result.status, 1);
    assert.match(result.stderr, /STAGE must be dev or prod/);
  });
}

for (const args of [[], ['deploy'], ['initialize', 'migrate']]) {
  test(`reject invalid operation arguments ${JSON.stringify(args)} before AWS`, (t) => {
    const result = runOperation(t, { args, plan: [] });
    assert.equal(result.status, 1);
    assert.match(result.stderr, /Usage:|Operation must be/);
  });
}

test('require an explicit region before AWS', (t) => {
  const result = runOperation(t, { env: { AWS_REGION: undefined }, plan: [] });
  assert.equal(result.status, 1);
  assert.match(result.stderr, /AWS_REGION must be set/);
});

for (const operation of ['migrate', 'initialize']) {
  test(`${operation} refuses an unreadable stack without leaking AWS errors`, (t) => {
    const plan = callPlan({ operation }).slice(0, 1);
    Object.assign(plan[0], { exitCode: 255, stdout: privateContent, stderr: privateContent });
    const result = runOperation(t, { operation, plan });
    assert.equal(result.status, 1);
    assert.match(result.stderr, /Cannot read application-dev-initialize.*cloudformation:DescribeStacks/);
  });

  for (const status of ['CREATE_IN_PROGRESS', 'UPDATE_IN_PROGRESS', 'UPDATE_ROLLBACK_IN_PROGRESS', 'UPDATE_ROLLBACK_FAILED', 'ROLLBACK_COMPLETE', 'CREATE_FAILED', 'DELETE_COMPLETE', undefined, privateContent]) {
    test(`${operation} refuses stack status ${JSON.stringify(status)} before Lambda calls`, (t) => {
      const stack = deployedStack();
      stack.Stacks[0].StackStatus = status;
      const plan = callPlan({ operation }).slice(0, 1);
      plan[0].stdout = JSON.stringify(stack);
      const result = runOperation(t, { operation, plan });
      assert.equal(result.status, 1);
      assert.match(result.stderr, /must be CREATE_COMPLETE.*inspect CloudFormation/);
    });
  }

  for (const [label, parameters] of [
    ['mismatched', [{ ParameterKey: 'CommitSHA', ParameterValue: otherSha }]],
    ['missing', []],
    ['null', null],
    ['malformed', [{ ParameterKey: 'CommitSHA', ParameterValue: privateContent }]],
    ['wrong type', [{ ParameterKey: 'CommitSHA', ParameterValue: 123 }]],
    ['wrong key', [{ ParameterKey: 'commitSHA', ParameterValue: sha }]],
    ['duplicate', [{ ParameterKey: 'CommitSHA', ParameterValue: sha }, { ParameterKey: 'CommitSHA', ParameterValue: sha }]],
  ]) {
    test(`${operation} refuses ${label} deployed CommitSHA before Lambda calls`, (t) => {
      const stack = deployedStack();
      stack.Stacks[0].Parameters = parameters;
      const plan = callPlan({ operation }).slice(0, 1);
      plan[0].stdout = JSON.stringify(stack);
      const result = runOperation(t, { operation, plan });
      assert.equal(result.status, 1);
      assert.match(result.stderr, /CommitSHA is missing, invalid or does not match.*No operation was invoked/);
    });
  }

  test(`${operation} stops on a failed Lambda update wait without logging its output`, (t) => {
    const plan = callPlan({ operation }).slice(0, 2);
    Object.assign(plan[1], { exitCode: 255, stdout: privateContent, stderr: privateContent });
    const result = runOperation(t, { operation, plan });
    assert.equal(result.status, 1);
    assert.match(result.stderr, /wait failed or timed out.*LastUpdateStatus.*lambda:GetFunctionConfiguration.*No operation was invoked/);
  });

  test(`${operation} reports uncertain completion on invocation transport failure`, (t) => {
    const plan = callPlan({ operation });
    Object.assign(plan[2], { exitCode: 255, stdout: privateContent, stderr: privateContent, result: privateContent });
    const result = runOperation(t, { operation, plan });
    assert.equal(result.status, 1);
    assert.match(result.stderr, /lambda:InvokeFunction.*Completion is unknown/);
  });

  for (const [label, metadata] of [
    ['FunctionError', { StatusCode: 200, FunctionError: privateContent }],
    ['empty FunctionError', { StatusCode: 200, FunctionError: '' }],
    ['async status', { StatusCode: 202 }],
    ['string status', { StatusCode: '200' }],
    ['missing status', {}],
    ['null', null],
    ['array', [{ StatusCode: 200 }]],
  ]) {
    test(`${operation} rejects ${label} metadata even with a successful result`, (t) => {
      const plan = callPlan({ operation });
      plan[2].stdout = JSON.stringify(metadata);
      const result = runOperation(t, { operation, plan });
      assert.equal(result.status, 1);
      assert.match(result.stderr, /did not confirm StatusCode 200 without FunctionError/);
    });
  }

  for (const [label, payload] of [
    ['missing', undefined],
    ['empty', ''],
    ['malformed', `{${privateContent}`],
    ['provider error', JSON.stringify({ errorMessage: privateContent })],
    ['success-shaped error', JSON.stringify({ status: 'ready', error: privateContent })],
    ['generic success', '{"success":true}'],
    ['number', '0'],
    ['array', '[]'],
    ['string', '"null"'],
    ['boolean', 'true'],
    ['non-JSON NaN', 'NaN'],
    ['non-JSON Infinity', 'Infinity'],
    ['multiple documents', operation === 'migrate' ? '{"status":"ready"}\n{"status":"ready"}' : 'null\nnull'],
    ['wrong operation result', operation === 'migrate' ? 'null' : '{"status":"ready"}'],
    ['trailing garbage', `${operation === 'migrate' ? '{"status":"ready"}' : 'null'} ${privateContent}`],
  ]) {
    test(`${operation} rejects ${label} result despite StatusCode 200`, (t) => {
      const plan = callPlan({ operation });
      if (payload === undefined) delete plan[2].result;
      else plan[2].result = payload;
      const result = runOperation(t, { operation, plan });
      assert.equal(result.status, 1);
      assert.match(result.stderr, operation === 'migrate' ? /Migration did not return exactly/ : /Initial FX capture did not return exactly JSON null/);
    });
  }
}

for (const [label, response] of [
  ['empty', ''],
  ['invalid JSON', privateContent],
  ['null', 'null'],
  ['missing Stacks', '{}'],
  ['missing stack', '{"Stacks":[]}'],
  ['null stack', '{"Stacks":[null]}'],
  ['multiple stacks', JSON.stringify({ Stacks: [...deployedStack().Stacks, ...deployedStack().Stacks] })],
  ['multiple documents', `${JSON.stringify(deployedStack())}\n${JSON.stringify(deployedStack())}`],
]) {
  test(`reject ${label} stack response`, (t) => {
    const plan = callPlan().slice(0, 1);
    plan[0].stdout = response;
    const result = runOperation(t, { plan });
    assert.equal(result.status, 1);
    assert.match(result.stderr, /Initialization stack.*must be CREATE_COMPLETE/);
  });
}

for (const response of ['', privateContent, '{"StatusCode":200}\n{"StatusCode":200}']) {
  test(`reject invalid invocation JSON ${JSON.stringify(response)}`, (t) => {
    const plan = callPlan();
    plan[2].stdout = response;
    const result = runOperation(t, { plan });
    assert.equal(result.status, 1);
    assert.match(result.stderr, /did not confirm StatusCode 200 without FunctionError/);
  });
}

test('successful standalone invocation does not require a GitHub summary', (t) => {
  const result = runOperation(t, { env: { GITHUB_STEP_SUMMARY: undefined } });
  assert.equal(result.status, 0, result.stderr);
  assert.equal(result.summary, '');
});

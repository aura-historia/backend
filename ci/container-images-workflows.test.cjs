'use strict';

const test = require('node:test');
const assert = require('node:assert/strict');
const { chmodSync, copyFileSync, mkdirSync, mkdtempSync, readFileSync, rmSync, symlinkSync, writeFileSync } = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const { spawnSync } = require('node:child_process');

const root = path.resolve(__dirname, '..');
const sha = '6a9c07fa66163b0917434cbf6bcc20e40b9c845c';
const firstDigest = `sha256:${'a'.repeat(64)}`;
const secondDigest = `sha256:${'b'.repeat(64)}`;

function workflowStep(file, name) {
  const lines = readFileSync(path.join(root, '.github/workflows', file), 'utf8').split('\n');
  const start = lines.indexOf(`      - name: ${name}`);
  assert.notEqual(start, -1, `Missing workflow step: ${name}`);
  const run = lines.indexOf('        run: |', start);
  const nextStep = lines.findIndex((line, index) => index > start && line.startsWith('      - '));
  assert.ok(run > start && (nextStep === -1 || run < nextStep), `Missing script for ${name}`);
  const body = [];
  for (let index = run + 1; index < lines.length && (lines[index].startsWith('          ') || lines[index] === ''); index += 1) {
    body.push(lines[index].slice(10));
  }
  return body.join('\n').replaceAll('${{ vars.AWS_REGION }}', 'us-east-1');
}

function run(command, args, cwd, env) {
  const result = spawnSync(command, args, { cwd, env, encoding: 'utf8' });
  assert.equal(result.status, 0, `${command} ${args.join(' ')} failed:\n${result.stdout}\n${result.stderr}`);
  return result.stdout;
}

function writeFixture(directory) {
  const mkdir = (relative) => mkdirSync(path.join(directory, relative), { recursive: true });
  mkdir('ci/container-images/periodic-matcher');
  mkdir('ci/container-images/second-image');
  mkdir('src/search-filter-periodic-match/src');
  mkdir('src/second-image/src');
  mkdir('.github/actions/resolve-container-images');
  mkdir('bin');
  copyFileSync(path.join(root, 'ci/container-images.cjs'), path.join(directory, 'ci/container-images.cjs'));
  copyFileSync(path.join(root, '.github/actions/resolve-container-images/resolve.mjs'), path.join(directory, '.github/actions/resolve-container-images/resolve.mjs'));
  writeFileSync(path.join(directory, 'src/search-filter-periodic-match/Cargo.toml'), '[package]\nname = "search-filter-periodic-match"\n');
  writeFileSync(path.join(directory, 'src/search-filter-periodic-match/src/main.rs'), 'fn main() {}\n');
  writeFileSync(path.join(directory, 'src/search-filter-periodic-match/Dockerfile'), 'FROM scratch\n');
  writeFileSync(path.join(directory, 'src/second-image/Cargo.toml'), '[package]\nname = "second-image"\n');
  writeFileSync(path.join(directory, 'src/second-image/src/main.rs'), 'fn main() {}\n');
  writeFileSync(path.join(directory, 'src/second-image/Dockerfile'), 'FROM scratch\n');
  const entries = [
    require('./container-images.json')[0],
    { id: 'second-image', crate: 'src/second-image', binary: 'second-image', dockerfile: 'src/second-image/Dockerfile', platform: 'linux/amd64', repository: 'aura-historia-second-image', digestParameter: 'SecondImageDigest', activationParameter: 'SecondImageEnabled', taskDefinitionOutput: 'SecondTaskDefinitionArn' },
  ];
  writeFileSync(path.join(directory, 'ci/container-images.json'), JSON.stringify(entries));
  for (const entry of entries) {
    const smoke = path.join(directory, `ci/container-images/${entry.id}/smoke.sh`);
    writeFileSync(smoke, `#!/usr/bin/env bash\nset -euo pipefail\nprintf '%s %s\\n' '${entry.id}' "$1" >> "$TEST_SMOKE_LOG"\n`);
    chmodSync(smoke, 0o755);
  }
  const mock = path.join(directory, 'bin/mock');
  writeFileSync(mock, `#!/usr/bin/env node
const fs = require('node:fs');
const path = require('node:path');
const tool = path.basename(process.argv[1]);
const args = process.argv.slice(2);
const log = process.env.TEST_LOG;
const events = fs.readFileSync(log, 'utf8').trim().split('\\n').filter(Boolean).map(JSON.parse);
fs.appendFileSync(log, JSON.stringify({ tool, args }) + '\\n');
if (tool === 'git' && args[0] === 'rev-parse') { console.log('${sha}'); process.exit(0); }
if (tool === 'npm') process.exit(0); // Never run CDK or deploy anything.
if (tool === 'docker') {
  if (args[0] === 'image' && args[1] === 'inspect') {
    console.log(JSON.stringify([{ Os: 'linux', Architecture: 'amd64', Config: { Labels: {
      'org.opencontainers.image.revision': process.env.DEPLOY_COMMIT_SHA,
      'org.opencontainers.image.source': process.env.GITHUB_SERVER_URL + '/' + process.env.GITHUB_REPOSITORY,
      'org.opencontainers.image.title': process.env.IMAGE_BINARY,
      'com.aura-historia.build-workflow': process.env.GITHUB_SERVER_URL + '/' + process.env.GITHUB_REPOSITORY + '/actions/runs/123/attempts/1',
    } } }]));
  }
  process.exit(0);
}
if (tool === 'aws') {
  if (args[0] === 'sts') { console.log('123456789012'); process.exit(0); }
  if (args[0] === 'ecr' && args[1] === 'get-login-password') { console.log('stub'); process.exit(0); }
  if (args[0] === 'ecr' && args[1] === 'describe-images') {
    const repository = args[args.indexOf('--repository-name') + 1];
    const tag = args[args.indexOf('--image-ids') + 1].slice('imageTag='.length);
    if (!events.some((event) => event.tool === 'docker' && event.args[0] === 'push' && event.args[1].endsWith('/' + repository + ':' + tag))) {
      console.error('ImageNotFoundException'); process.exit(254);
    }
    const digest = repository === 'aura-historia-second-image' ? '${secondDigest}' : '${firstDigest}';
    console.log(JSON.stringify({ imageDetails: [{ imageDigest: digest, imageTags: [tag] }] }));
    process.exit(0);
  }
  if (args[0] === 'cloudformation' && args[1] === 'describe-stacks') {
    if (args.includes('--query')) { console.log('UPDATE_COMPLETE'); process.exit(0); }
    console.log(JSON.stringify({ Stacks: [{ Outputs: [
      { OutputKey: 'PeriodicMatcherTaskDefinitionArn', OutputValue: 'arn:aws:ecs:us-east-1:123456789012:task-definition/matcher:1' },
      { OutputKey: 'SecondTaskDefinitionArn', OutputValue: 'arn:aws:ecs:us-east-1:123456789012:task-definition/second:1' },
    ] }] }));
    process.exit(0);
  }
  if (args[0] === 'lambda' && args[1] === 'wait') process.exit(0);
  if (args[0] === 'lambda' && args[1] === 'invoke') {
    fs.writeFileSync('migration-result.json', '{"status":"ready"}');
    console.log(JSON.stringify({ StatusCode: 200 }));
    process.exit(0);
  }
}
console.error('Unexpected mocked command: ' + tool + ' ' + args.join(' '));
process.exit(1);
`);
  chmodSync(mock, 0o755);
  for (const tool of ['aws', 'docker', 'git', 'npm']) symlinkSync(mock, path.join(directory, 'bin', tool));
  writeFileSync(path.join(directory, 'commands.log'), '');
  writeFileSync(path.join(directory, 'smoke.log'), '');
  writeFileSync(path.join(directory, 'github-output'), '');
  writeFileSync(path.join(directory, 'summary'), '');
  return entries;
}

test('every deployment reconciles artifact repositories before publishing and deploying', () => {
  const workflow = readFileSync(path.join(root, '.github/workflows/deploy.yml'), 'utf8');
  const artifactJob = workflow.split('  aws-container-artifacts:')[1].split('  aws-push-container-images:')[0];
  const publisherJob = workflow.split('  aws-push-container-images:')[1].split('  aws-push-lambda:')[0];
  const deployJob = workflow.split('  aws-cdk-deploy:')[1];
  assert.doesNotMatch(workflow, /reconcile_artifact_stack|Detect repository definition changes/);
  assert.match(artifactJob, /needs: \[infra-test\]/);
  assert.doesNotMatch(artifactJob, /^    if:/m);
  assert.match(artifactJob, /github\.event_name == 'workflow_dispatch' && inputs\.stage/);
  assert.match(publisherJob, /if: github\.event_name == 'push'/);
  assert.match(publisherJob, /needs: \[infra-test, aws-container-artifacts\]/);
  assert.match(deployJob, /needs\.aws-container-artifacts\.result == 'success'/);
  assert.match(deployJob, /github\.event_name == 'workflow_dispatch'[\s\S]*needs\.aws-push-container-images\.result == 'skipped'/);
  assert.match(deployJob, /aws-push-container-images,/);
  const resolve = deployJob.indexOf('uses: ./.github/actions/resolve-container-images');
  const preflight = deployJob.indexOf('name: Preflight stage artifacts and deployed stack state');
  const update = deployJob.indexOf('name: Update artifact SHA and image digests with previous CloudFormation templates');
  assert.ok(resolve >= 0 && preflight > resolve && update > preflight);
});

test('a second catalog image builds, publishes, resolves, and reaches deploy without contacting AWS', () => {
  const directory = mkdtempSync(path.join(os.tmpdir(), 'container-workflows-'));
  try {
    const entries = writeFixture(directory);
    const env = {
      ...process.env,
      PATH: `${path.join(directory, 'bin')}${path.delimiter}${process.env.PATH}`,
      TEST_LOG: path.join(directory, 'commands.log'),
      TEST_SMOKE_LOG: path.join(directory, 'smoke.log'),
      GITHUB_WORKSPACE: directory,
      GITHUB_OUTPUT: path.join(directory, 'github-output'),
      GITHUB_STEP_SUMMARY: path.join(directory, 'summary'),
      GITHUB_SHA: sha,
      DEPLOY_COMMIT_SHA: sha,
      CONTAINER_COMMIT_SHA: sha,
      GITHUB_SERVER_URL: 'https://github.com',
      GITHUB_REPOSITORY: 'org/repo',
      GITHUB_RUN_ID: '123',
      GITHUB_RUN_ATTEMPT: '1',
      STAGE: 'dev',
      STACK_NAME_PREFIX: 'application-dev',
    };
    const catalogWorkflow = readFileSync(path.join(root, '.github/workflows/container-images.yml'), 'utf8');
    const deployWorkflow = readFileSync(path.join(root, '.github/workflows/deploy.yml'), 'utf8');
    const initializeWorkflow = readFileSync(path.join(root, '.github/workflows/initialize.yml'), 'utf8');
    assert.match(catalogWorkflow, /matrix: \$\{\{ fromJSON\(needs\.catalog\.outputs\.container_matrix\) \}\}/);
    assert.match(deployWorkflow, /matrix: \$\{\{ fromJSON\(needs\.infra-test\.outputs\.container_matrix\) \}\}/);
    assert.match(deployWorkflow, /needs\.aws-push-container-images\.result == 'success'/);
    const publisherJob = deployWorkflow.split('  aws-push-container-images:')[1].split('  aws-push-lambda:')[0];
    assert.doesNotMatch(publisherJob, /^    environment:/m, 'immutable image publication must not require a separate environment approval');
    for (const workflow of [deployWorkflow, initializeWorkflow]) {
      assert.match(workflow, /uses: \.\/\.github\/actions\/resolve-container-images/);
      assert.match(workflow, /CONTAINER_IMAGE_DIGESTS: \$\{\{ steps\.container-images\.outputs\.digests \}\}/);
    }
    const build = workflowStep('container-images.yml', 'Build local image');
    assert.match(build, /bash ci\/container-images\/\$\{IMAGE_ID\}\/smoke\.sh "\$IMAGE"/);
    assert.doesNotMatch(build, /periodic-matcher|search-filter-periodic-match|matcher-smoke/);
    run(process.execPath, ['ci/container-images.cjs', 'validate'], directory, env);
    const matrix = JSON.parse(run(process.execPath, ['ci/container-images.cjs', 'matrix'], directory, env).trim().slice('matrix='.length)).include;
    assert.deepEqual(matrix, entries);
    for (const entry of matrix) {
      const imageEnv = { ...env, IMAGE_ID: entry.id, IMAGE_BINARY: entry.binary, IMAGE_PLATFORM: entry.platform, IMAGE_DOCKERFILE: entry.dockerfile, IMAGE_REPOSITORY: entry.repository };
      run('bash', ['-e', '-c', build], directory, imageEnv);
      run('bash', ['-e', '-c', workflowStep('deploy.yml', 'Publish tested image or reuse immutable SHA tag')], directory, imageEnv);
    }
    assert.deepEqual(readFileSync(path.join(directory, 'smoke.log'), 'utf8').trim().split('\n'), matrix.flatMap((entry) => [
      `${entry.id} local/${entry.id}:${sha}`,
      `${entry.id} local/${entry.id}:${sha}-123-1`,
    ]));
    const pushes = readFileSync(path.join(directory, 'commands.log'), 'utf8').trim().split('\n').map(JSON.parse)
      .filter((event) => event.tool === 'docker' && event.args[0] === 'push');
    assert.equal(pushes.length, 2);
    for (const entry of entries) assert.ok(pushes.some((event) => event.args[1].endsWith(`/${entry.repository}:git-${sha}`)));

    run(process.execPath, ['.github/actions/resolve-container-images/resolve.mjs'], directory, env);
    const digests = JSON.parse(readFileSync(env.GITHUB_OUTPUT, 'utf8').trim().slice('digests='.length));
    assert.deepEqual(digests, { PeriodicMatcherImageDigest: firstDigest, SecondImageDigest: secondDigest });
    const stack = path.join(directory, 'compute-stack.json');
    const digestFile = path.join(directory, 'digests.json');
    writeFileSync(stack, '');
    writeFileSync(digestFile, JSON.stringify(digests));
    const initialized = JSON.parse(run(process.execPath, ['ci/container-images.cjs', 'initialize-update', '--stack-file', stack, '--commit-sha', sha, '--digests', digestFile], directory, env));
    assert.deepEqual(initialized.parameters, { CommitSHA: sha, ...digests });

    run('bash', ['-e', '-c', workflowStep('deploy.yml', 'Deploy foundation or migrate and update initialized application')], directory, { ...env, CONTAINER_IMAGE_DIGESTS: JSON.stringify(digests) });
    const commands = readFileSync(env.TEST_LOG, 'utf8').trim().split('\n').map(JSON.parse);
    const compute = commands.filter((event) => event.tool === 'npm' && event.args.includes('application-dev-compute'));
    assert.equal(compute.length, 1);
    for (const [parameter, value] of Object.entries(digests)) {
      assert.ok(compute[0].args.includes(`application-dev-compute:${parameter}=${value}`));
    }
    assert.match(readFileSync(env.GITHUB_STEP_SUMMARY, 'utf8'), /second-image: arn:aws:ecs:/);
  } finally {
    rmSync(directory, { recursive: true, force: true });
  }
});

'use strict';

const test = require('node:test');
const assert = require('node:assert/strict');
const { chmodSync, copyFileSync, mkdirSync, mkdtempSync, readdirSync, readFileSync, rmSync, symlinkSync, writeFileSync } = require('node:fs');
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
  return body.join('\n');
}

// No command falls through to an installed AWS, CDK, Docker, or Git executable.
// Unexpected commands are recorded separately from intentional fixture failures.
function mockTool() {
  const assert = require('node:assert/strict');
  const fs = require('node:fs');
  const path = require('node:path');
  const env = process.env;
  const tool = path.basename(process.argv[1]);
  const args = process.argv.slice(2);
  const event = { tool, args };
  const config = JSON.parse(fs.readFileSync('fixture.json', 'utf8'));
  const entries = JSON.parse(fs.readFileSync('ci/container-images.json', 'utf8'));
  const events = fs.readFileSync(env.TEST_LOG, 'utf8').trim().split('\n').filter(Boolean).map(JSON.parse);
  const registry = JSON.parse(fs.readFileSync('registry.json', 'utf8'));
  const tag = `git-${env.DEPLOY_COMMIT_SHA}`;
  const registryHost = `123456789012.dkr.ecr.${env.AWS_REGION}.amazonaws.com`;
  const source = `${env.GITHUB_SERVER_URL}/${env.GITHUB_REPOSITORY}`;
  const workflow = `${source}/actions/runs/${env.GITHUB_RUN_ID}/attempts/${env.GITHUB_RUN_ATTEMPT}`;
  const prefix = `application-${env.STAGE}`;
  const currentSha = config.computeCommitSha ?? 'c'.repeat(40);
  const stackNames = ['network', 'data', 'initialize', 'compute', 'api', ...(env.STAGE === 'prod' ? ['observability'] : [])];
  const failure = (stderr) => ({ stderr, status: 1 });
  const json = (value) => ({ stdout: JSON.stringify(value) });
  const imageRef = (entry) => `${registryHost}/${entry.repository}@${config.digests[entry.digestParameter]}`;
  function handle() {
    if (tool === 'git') {
      if (args[0] === 'rev-parse') {
        assert.deepEqual(args, ['rev-parse', 'HEAD']);
        return { stdout: env.DEPLOY_COMMIT_SHA };
      }
      assert.equal(env.DEPLOY_SCOPE, 'auto', 'Explicit scopes must not compare migration sources');
      assert.deepEqual(args, [
        '--no-pager', 'diff', '--quiet', currentSha, env.DEPLOY_COMMIT_SHA,
        '--', 'migrations', 'infra/sql', 'src/database-migration-lambda',
      ]);
      return { status: config.gitDiffExitCode ?? 0 };
    }
    if (tool === 'smoke') {
      const entry = entries.find((entry) => entry.id === args[0]);
      assert.ok(entry);
      assert.equal(args.length, 2);
      assert.ok([
        `local/${entry.id}:${env.DEPLOY_COMMIT_SHA}`,
        `local/${entry.id}:${env.DEPLOY_COMMIT_SHA}-${env.GITHUB_RUN_ID}-${env.GITHUB_RUN_ATTEMPT}`,
        imageRef(entry),
      ].includes(args[1]));
      return config.smokeFailure === true || (config.smokeFailure === 'remote' && args[1].includes('@'))
        ? failure('Image-owned smoke test failed') : {};
    }
    if (tool === 'ensure-dms-vpc-role') {
      assert.deepEqual(args, []);
      return {};
    }
    if (tool === 'npm') {
      assert.deepEqual(args.slice(0, 6), ['--prefix', 'infra', 'run', 'cdk', '--', 'deploy']);
      const stack = stackNames.find((name) => args[6] === `${prefix}-${name}`);
      assert.ok(stack, 'Unknown deployment stack');
      const parameters = stack === 'compute' ? { CommitSHA: env.DEPLOY_COMMIT_SHA, ...config.digests }
        : stack === 'initialize' ? { CommitSHA: env.DEPLOY_COMMIT_SHA } : {};
      assert.deepEqual(args.slice(7), [
        ...Object.entries(parameters).flatMap(([key, value]) => ['--parameters', `${prefix}-${stack}:${key}=${value}`]),
        '--context', `stage=${env.STAGE}`, '--context', `stackNamePrefix=${prefix}`,
        '--require-approval', 'never', '--method', 'change-set', '--exclusively', '--previous-parameters',
        '--no-path-metadata', '--no-asset-metadata', '--no-version-reporting', '--no-notices',
      ]);
      return {};
    }
    if (tool === 'aws') {
      if (args[0] === 'sts') {
        assert.deepEqual(args, ['sts', 'get-caller-identity', '--query', 'Account', '--output', 'text']);
        return { stdout: '123456789012' };
      }
      if (args[0] === 'ecr' && args[1] === 'describe-repositories') {
        const entry = entries.find((entry) => entry.repository === env.IMAGE_REPOSITORY);
        assert.ok(entry, 'Unknown ECR repository');
        assert.deepEqual(args, ['ecr', 'describe-repositories', '--repository-names', entry.repository, '--output', 'json']);
        if (config.repositoryFailure) return failure(config.repositoryFailure);
        if (config.repositoryResponse !== undefined) return { stdout: config.repositoryResponse };
        const repository = {
          registryId: '123456789012',
          repositoryName: entry.repository,
          repositoryUri: `${registryHost}/${entry.repository}`,
          imageTagMutability: 'IMMUTABLE',
          encryptionConfiguration: { encryptionType: 'AES256' },
          ...config.repository,
        };
        return json({ repositories: Array(config.repositoryCount ?? 1).fill(repository) });
      }
      if (args[0] === 'ecr' && args[1] === 'get-login-password') {
        assert.deepEqual(args, ['ecr', 'get-login-password']);
        return { stdout: 'mock-password' };
      }
      if (args[0] === 'ecr' && args[1] === 'describe-images') {
        const entry = entries.find((entry) => entry.repository === args[3]);
        assert.ok(entry, 'Unknown ECR repository');
        assert.deepEqual(args, ['ecr', 'describe-images', '--repository-name', entry.repository, '--image-ids', `imageTag=${tag}`, '--output', 'json']);
        if (config.ecrFailure) return failure(`An error occurred (${config.ecrFailure}) when calling DescribeImages`);
        if (!registry[entry.repository]) return failure('An error occurred (ImageNotFoundException) when calling DescribeImages');
        return json({ imageDetails: [{
          imageDigest: config.ecrResponse === 'invalid-digest' ? 'sha256:bad' : registry[entry.repository].digest,
          imageTags: [config.ecrResponse === 'wrong-tag' ? 'latest' : tag],
        }] });
      }
      if (args[0] === 'cloudformation' && args[1] === 'describe-stacks') {
        const stack = stackNames.find((name) => args[3] === `${prefix}-${name}`);
        assert.ok(stack, 'Unknown CloudFormation stack');
        assert.deepEqual(args, ['cloudformation', 'describe-stacks', '--stack-name', `${prefix}-${stack}`, '--output', 'json']);
        if (config.deniedStack === stack) return failure('An error occurred (AccessDeniedException) when calling DescribeStacks');
        const deployed = events.some((event) => event.tool === 'npm' && event.args.includes(`${prefix}-${stack}`));
        if (config.absentStacks?.includes(stack) && !deployed) {
          return failure(`An error occurred (ValidationError) when calling DescribeStacks: Stack with id ${prefix}-${stack} does not exist`);
        }
        return json({ Stacks: [{
          StackStatus: config.stackStatuses?.[stack] ?? 'UPDATE_COMPLETE',
          Parameters: [
            ...(stack === 'compute' && config.computeCommitSha === null ? [] : [{
              ParameterKey: 'CommitSHA', ParameterValue: stack === 'compute' ? currentSha : 'c'.repeat(40),
            }]),
            ...entries.map((entry) => ({ ParameterKey: entry.digestParameter, ParameterValue: `sha256:${'c'.repeat(64)}` })),
            ...entries.filter((entry) => entry.activationParameter).map((entry, index) => ({
              ParameterKey: entry.activationParameter, ParameterValue: index === 0 ? 'true' : 'false',
            })),
            { ParameterKey: 'CdcRouterEnabled', ParameterValue: 'true' },
          ],
          Outputs: entries.map((entry) => ({
            OutputKey: entry.taskDefinitionOutput,
            OutputValue: `arn:aws:ecs:${env.AWS_REGION}:123456789012:task-definition/${entry.id}:1`,
          })),
        }] });
      }
    }
    if (tool === 'docker') {
      const entry = entries.find((entry) => entry.id === env.IMAGE_ID);
      assert.ok(entry);
      const uri = `${registryHost}/${entry.repository}`;
      const local = `local/${entry.id}:${env.DEPLOY_COMMIT_SHA}-${env.GITHUB_RUN_ID}-${env.GITHUB_RUN_ATTEMPT}`;
      if (args[0] === 'login') {
        assert.deepEqual(args, ['login', '--username', 'AWS', '--password-stdin', registryHost]);
        assert.equal(fs.readFileSync(0, 'utf8').trim(), 'mock-password');
        return {};
      }
      if (args[0] === 'build') {
        const ref = args.at(-2);
        assert.ok([local, `local/${entry.id}:${env.DEPLOY_COMMIT_SHA}`].includes(ref));
        assert.deepEqual(args, [
          'build', '--platform', entry.platform, '-f', entry.dockerfile,
          '--build-arg', `SOURCE_REVISION=${env.DEPLOY_COMMIT_SHA}`, '--build-arg', `BUILD_WORKFLOW_IDENTITY=${workflow}`,
          '--label', `org.opencontainers.image.source=${source}`, '--label', `org.opencontainers.image.revision=${env.DEPLOY_COMMIT_SHA}`,
          '--label', `org.opencontainers.image.title=${entry.binary}`, '--label', `com.aura-historia.build-workflow=${workflow}`,
          '-t', ref, '.',
        ]);
        return {};
      }
      if (args[0] === 'image' && args[1] === 'inspect') {
        assert.equal(args.length, 3);
        const remote = args[2] === imageRef(entry);
        assert.ok(remote || args[2] === local);
        const metadata = {
          Os: 'linux', Architecture: entry.platform.split('/')[1], Config: { Labels: {
            'org.opencontainers.image.revision': env.DEPLOY_COMMIT_SHA,
            'org.opencontainers.image.source': source,
            'org.opencontainers.image.title': entry.binary,
            'com.aura-historia.build-workflow': remote ? registry[entry.repository].workflow : workflow,
          } },
        };
        if (config.badMetadata && (config.badMetadataPhase === 'local' ? !remote : remote)) {
          if (['Os', 'Architecture'].includes(config.badMetadata)) metadata[config.badMetadata] = 'invalid';
          else metadata.Config.Labels[config.badMetadata] = 'untrusted';
        }
        return json([metadata]);
      }
      if (args[0] === 'pull') {
        assert.deepEqual(args, ['pull', '--platform', entry.platform, imageRef(entry)]);
        assert.ok(registry[entry.repository]);
        return {};
      }
      if (args[0] === 'tag') {
        assert.deepEqual(args, ['tag', local, `${uri}:${tag}`]);
        return {};
      }
      if (args[0] === 'push') {
        assert.deepEqual(args, ['push', `${uri}:${tag}`]);
        assert.equal(registry[entry.repository], undefined, 'An existing immutable tag must never be overwritten');
        assert.ok(events.some((event) => event.tool === 'smoke' && event.args[1] === local), 'Smoke must precede publication');
        if (config.pushFailure === 'missing') return failure('Registry push failed');
        registry[entry.repository] = {
          digest: config.digests[entry.digestParameter],
          workflow: config.pushFailure === 'competing' ? `${source}/actions/runs/456/attempts/2` : workflow,
        };
        fs.writeFileSync('registry.json', JSON.stringify(registry));
        return config.pushFailure === 'competing' ? failure('ImageTagAlreadyExistsException') : {};
      }
    }
    throw new Error(`Unexpected mocked command: ${tool} ${args.join(' ')}`);
  }
  try {
    const result = handle();
    if (result.stdout !== undefined) process.stdout.write(`${result.stdout}\n`);
    if (result.stderr) process.stderr.write(`${result.stderr}\n`);
    process.exitCode = result.status ?? 0;
  } catch (error) {
    event.unexpected = error.message;
    process.stderr.write(`${error.stack}\n`);
    process.exitCode = 99;
  } finally {
    fs.appendFileSync(env.TEST_LOG, `${JSON.stringify(event)}\n`);
  }
}

function fixture(t, options = {}) {
  const directory = mkdtempSync(path.join(os.tmpdir(), 'container-workflows-'));
  t.after(() => rmSync(directory, { recursive: true, force: true }));
  const mkdir = (relative) => mkdirSync(path.join(directory, relative), { recursive: true });
  const write = (relative, text) => writeFileSync(path.join(directory, relative), text);
  for (const dir of ['ci', 'infra/scripts', '.github/actions/resolve-container-images', 'bin']) mkdir(dir);
  for (const file of ['ci/container-images.cjs', 'ci/publish-container.sh', 'ci/deploy-stacks.sh', '.github/actions/resolve-container-images/resolve.mjs']) {
    copyFileSync(path.join(root, file), path.join(directory, file));
  }
  const entries = [
    require('./container-images.json')[0],
    { id: 'second-image', crate: 'src/second-image', binary: 'second-image', dockerfile: 'src/second-image/Dockerfile', platform: 'linux/arm64', repository: 'aura-historia-second-image', digestParameter: 'SecondImageDigest', activationParameter: 'SecondImageEnabled', taskDefinitionOutput: 'SecondTaskDefinitionArn' },
  ];
  for (const entry of entries) {
    mkdir(`${entry.crate}/src`);
    mkdir(`ci/container-images/${entry.id}`);
    write(`${entry.crate}/Cargo.toml`, `[package]\nname = "${entry.binary}"\n`);
    write(`${entry.crate}/src/main.rs`, 'fn main() {}\n');
    write(entry.dockerfile, 'FROM scratch\n');
    const smoke = `ci/container-images/${entry.id}/smoke.sh`;
    write(smoke, `#!/usr/bin/env bash\nset -euo pipefail\nsmoke '${entry.id}' "$@"\n`);
    chmodSync(path.join(directory, smoke), 0o755);
  }
  write('ci/container-images.json', JSON.stringify(entries));
  write('infra/scripts/ensure-dms-vpc-role.sh', '#!/usr/bin/env bash\nset -euo pipefail\nensure-dms-vpc-role "$@"\n');
  write('bin/mock', `#!/usr/bin/env node\n(${mockTool.toString()})();\n`);
  chmodSync(path.join(directory, 'bin/mock'), 0o755);
  for (const tool of ['aws', 'docker', 'git', 'npm', 'smoke', 'ensure-dms-vpc-role']) symlinkSync('mock', path.join(directory, 'bin', tool));
  for (const file of ['commands.log', 'github-output', 'summary']) write(file, '');
  const digests = { [entries[0].digestParameter]: firstDigest, [entries[1].digestParameter]: secondDigest };
  write('fixture.json', JSON.stringify({ ...options, digests }));
  write('registry.json', JSON.stringify(options.existingImages ? Object.fromEntries(entries.map((entry) => [entry.repository, {
    digest: digests[entry.digestParameter], workflow: 'https://github.com/org/repo/actions/runs/100/attempts/1',
  }])) : {}));
  // Deliberately do not inherit AWS credentials, SDK configuration, or shell startup hooks.
  const env = {
    PATH: `${path.join(directory, 'bin')}${path.delimiter}${process.env.PATH}`,
    HOME: directory,
    TEST_LOG: path.join(directory, 'commands.log'),
    GITHUB_WORKSPACE: directory,
    GITHUB_OUTPUT: path.join(directory, 'github-output'),
    GITHUB_STEP_SUMMARY: path.join(directory, 'summary'),
    GITHUB_SHA: sha,
    DEPLOY_COMMIT_SHA: sha,
    CONTAINER_COMMIT_SHA: sha,
    CONTAINER_IMAGE_DIGESTS: JSON.stringify(digests),
    GITHUB_SERVER_URL: 'https://github.com',
    GITHUB_REPOSITORY: 'org/repo',
    GITHUB_RUN_ID: '123',
    GITHUB_RUN_ATTEMPT: '1',
    AWS_REGION: 'eu-central-1',
    STAGE: 'dev',
    DEPLOY_SCOPE: 'auto',
  };
  const read = (file) => readFileSync(path.join(directory, file), 'utf8');
  const events = () => read('commands.log').trim().split('\n').filter(Boolean).map(JSON.parse);
  const invoke = (command, args, extraEnv = {}) => {
    const result = spawnSync(command, args, { cwd: directory, env: { ...env, ...extraEnv }, encoding: 'utf8', timeout: 15000 });
    assert.equal(result.error, undefined);
    for (const event of events()) assert.equal(event.unexpected, undefined, event.unexpected);
    return result;
  };
  const run = (command, args, extraEnv = {}) => {
    const result = invoke(command, args, extraEnv);
    assert.equal(result.status, 0, `${command} ${args.join(' ')} failed:\n${result.stdout}\n${result.stderr}`);
    return result.stdout;
  };
  const imageEnv = (entry) => ({ IMAGE_ID: entry.id, IMAGE_BINARY: entry.binary, IMAGE_PLATFORM: entry.platform, IMAGE_DOCKERFILE: entry.dockerfile, IMAGE_REPOSITORY: entry.repository });
  const publish = (entry = entries[1]) => invoke('bash', ['ci/publish-container.sh'], imageEnv(entry));
  const deploy = (extraEnv) => invoke('bash', ['ci/deploy-stacks.sh'], extraEnv);
  return { entries, digests, read, events, run, imageEnv, publish, deploy };
}

function assertDeployments(f, expected, stage = 'dev') {
  const events = f.events();
  const deployments = events.filter((event) => event.tool === 'npm');
  assert.deepEqual(deployments.map((event) => event.args[6]), expected.map((stack) => `application-${stage}-${stack}`));
  assert.equal(events.filter((event) => event.tool === 'ensure-dms-vpc-role').length, 1);
  for (const event of deployments) {
    assert.ok(event.args.includes('--previous-parameters'));
    assert.ok(event.args.includes('--exclusively'));
    assert.equal(event.args[event.args.indexOf('--method') + 1], 'change-set');
    for (const key of ['CdcRouterEnabled', ...f.entries.map((entry) => entry.activationParameter)]) {
      assert.ok(!event.args.some((arg) => arg.includes(`${key}=`)), `Activation override: ${key}`);
    }
    assert.ok(!event.args.some((arg) => /import|hotswap/.test(arg)));
  }
  assert.ok(events.filter((event) => event.tool === 'aws').every((event) => !['lambda', 'ecs'].includes(event.args[0])));
  return deployments;
}

function assertNoMutation(f, expectedGitDiffs = 0) {
  const events = f.events();
  assert.ok(events.every((event) =>
    (event.tool === 'aws' && event.args[0] === 'cloudformation' && event.args[1] === 'describe-stacks') ||
    (event.tool === 'git' && event.args[0] === '--no-pager' && event.args[1] === 'diff')));
  assert.equal(events.filter((event) => event.tool === 'git').length, expectedGitDiffs);
  assert.equal(f.read('summary'), '');
}

test('deployment publishers use the release catalogs on every trigger before the only environment gate at final deploy', () => {
  const workflow = readFileSync(path.join(root, '.github/workflows/deploy.yml'), 'utf8');
  const jobs = Object.fromEntries([...workflow.split('\njobs:\n')[1].matchAll(/^  ([\w-]+):\n([\s\S]*?)(?=^  [\w-]+:\n|(?![\s\S]))/gm)].map((match) => [match[1], match[2]]));
  const publishers = ['aws-push-container-images', 'aws-push-lambda', 'aws-push-mail-templates'];
  assert.deepEqual(Object.keys(jobs), ['infra-test', ...publishers, 'aws-cdk-deploy']);
  const needs = (job) => job.match(/^    needs:\s*\[([^\]]+)\]/m)?.[1].split(',').map((name) => name.trim()).filter(Boolean);
  for (const name of publishers) assert.deepEqual(needs(jobs[name]), ['infra-test'], `${name} must depend only on infrastructure tests`);
  assert.deepEqual(new Set(needs(jobs['aws-cdk-deploy'])), new Set(['infra-test', ...publishers]));
  assert.deepEqual(Object.entries(jobs).filter(([, job]) => /^    environment:/m.test(job)).map(([name]) => name), ['aws-cdk-deploy']);
  assert.match(jobs['aws-cdk-deploy'], /^    environment:\n      name: aws-\$\{\{ needs\.infra-test\.outputs\.stage \}\}$/m);
  const publisher = jobs['aws-push-container-images'];
  assert.match(publisher, /matrix: \$\{\{ fromJSON\(needs\.infra-test\.outputs\.container_matrix\) \}\}/);
  assert.match(publisher, /run: bash ci\/publish-container\.sh/);
  assert.match(jobs['infra-test'], /^      stage: \$\{\{ env\.STAGE \}\}$/m);
  for (const [name, job] of Object.entries(jobs).filter(([name]) => name.startsWith('aws-'))) {
    assert.doesNotMatch(job, /^    if:.*github\.event_name == 'push'/m, `${name} must not be push-only`);
    assert.match(job, /^      DEPLOY_COMMIT_SHA: \$\{\{ needs\.infra-test\.outputs\.commit_sha \}\}$/m, `${name} must select the resolved release SHA`);
    assert.match(job, /^          ref: \$\{\{ env\.DEPLOY_COMMIT_SHA \}\}$/m, `${name} must check out the selected release`);
    assert.equal([...job.matchAll(/uses: actions\/checkout@/g)].length, 1, `${name} must not check out another source`);
  }
  assert.match(jobs['infra-test'], /^      lambda_matrix: \$\{\{ steps\.lambda-catalog\.outputs\.matrix \}\}$/m);
  assert.ok(jobs['infra-test'].includes('require("./ci/lambda-binaries.json")'));
  assert.match(jobs['aws-push-lambda'], /matrix: \$\{\{ fromJSON\(needs\.infra-test\.outputs\.lambda_matrix\) \}\}/);
  for (const field of ['id', 'binary', 'repository', 'dockerfile', 'platform']) {
    assert.ok(publisher.includes(`IMAGE_${field.toUpperCase()}: \${{ matrix.${field} }}`));
  }
  assert.match(jobs['aws-cdk-deploy'], /^          ref: \$\{\{ env\.DEPLOY_COMMIT_SHA \}\}\n          fetch-depth: 0$/m);
  assert.match(jobs['aws-cdk-deploy'], /run: bash ci\/deploy-stacks\.sh/);
  assert.match(jobs['aws-cdk-deploy'], /CONTAINER_IMAGE_DIGESTS: \$\{\{ steps\.container-images\.outputs\.digests \}\}/);
});

test('no workflow reconciles, creates, imports, or configures container repositories', () => {
  for (const file of readdirSync(path.join(root, '.github/workflows')).filter((file) => /\.ya?ml$/.test(file))) {
    const workflow = readFileSync(path.join(root, '.github/workflows', file), 'utf8');
    assert.doesNotMatch(workflow, /aws-container-artifacts|aws-container-artifact-stack|aura-historia-container-artifacts|bin\/artifacts\.ts/, file);
    assert.doesNotMatch(workflow, /\b(?:create-repository|delete-repository|put-image-tag-mutability|put-image-scanning-configuration|put-registry-scanning-configuration|set-repository-policy|delete-repository-policy|put-lifecycle-policy|delete-lifecycle-policy)\b/, file);
    assert.doesNotMatch(workflow, /--import-existing-resources|\bcdk\s+(?:--\s+)?import\b/, file);
  }
});

function assertRepositoryPreflight(f, entry = f.entries[1]) {
  assert.deepEqual(f.events().slice(0, 2), [
    { tool: 'aws', args: ['sts', 'get-caller-identity', '--query', 'Account', '--output', 'text'] },
    { tool: 'aws', args: ['ecr', 'describe-repositories', '--repository-names', entry.repository, '--output', 'json'] },
  ]);
}

function assertRepositoryValidationOnly(f) {
  assertRepositoryPreflight(f);
  assert.equal(f.events().length, 2, 'Repository validation must fail before Docker login, build, push, or any repository mutation');
  assert.equal(f.read('summary'), '');
}

for (const existingImages of [false, true]) {
  test(`publication ${existingImages ? 'reuses a trusted image' : 'builds a missing image'} in an existing repository without CloudFormation ownership checks`, (t) => {
    const f = fixture(t, { existingImages });
    const result = f.publish();
    assert.equal(result.status, 0, result.stderr);
    assertRepositoryPreflight(f);
    assert.ok(f.events().every((event) => ['aws', 'docker', 'smoke'].includes(event.tool)));
    assert.ok(f.events().filter((event) => event.tool === 'aws').every((event) => ['sts', 'ecr'].includes(event.args[0])));
    assert.equal(f.events().filter((event) => event.tool === 'docker' && event.args[0] === 'push').length, existingImages ? 0 : 1);
    assert.ok(f.read('summary').includes(`- Registry digest: ${secondDigest}`));
  });
}

for (const scanOnPush of [false, true]) {
  test(`repository scanOnPush=${scanOnPush} does not change the publication contract`, (t) => {
    const f = fixture(t, { existingImages: true, repository: { imageScanningConfiguration: { scanOnPush }, imageTagMutabilityExclusionFilters: [] } });
    const result = f.publish();
    assert.equal(result.status, 0, result.stderr);
    assertRepositoryPreflight(f);
  });
}

for (const repositoryFailure of [
  'An error occurred (RepositoryNotFoundException) when calling DescribeRepositories: repository does not exist',
  'An error occurred (AccessDeniedException) when calling DescribeRepositories: not authorized',
  'Could not connect to the endpoint URL: https://api.ecr.eu-central-1.amazonaws.com',
]) {
  test(`repository lookup fails closed without hiding the AWS error: ${repositoryFailure}`, (t) => {
    const f = fixture(t, { repositoryFailure });
    const result = f.publish();
    assert.notEqual(result.status, 0);
    assert.ok(result.stderr.includes(repositoryFailure));
    assert.match(result.stderr, /Cannot read ECR repository aura-historia-second-image in 123456789012\/eu-central-1/);
    assert.match(result.stderr, /if missing, provision it using infra\/README\.md#container-repository-setup; otherwise inspect the AWS error/);
    assertRepositoryValidationOnly(f);
  });
}

for (const { name, options } of [
  { name: 'malformed JSON', options: { repositoryResponse: '{broken' } },
  { name: 'empty output', options: { repositoryResponse: '' } },
  { name: 'missing repositories', options: { repositoryResponse: '{}' } },
  { name: 'null repositories', options: { repositoryResponse: '{"repositories":null}' } },
  { name: 'non-array repositories', options: { repositoryResponse: '{"repositories":{}}' } },
  { name: 'zero repositories', options: { repositoryCount: 0 } },
  { name: 'multiple repositories', options: { repositoryCount: 2 } },
  { name: 'wrong registry account', options: { repository: { registryId: '210987654321' } } },
  { name: 'missing registry account', options: { repository: { registryId: null } } },
  { name: 'wrong repository name', options: { repository: { repositoryName: 'aura-historia-other' } } },
  { name: 'wrong URI account', options: { repository: { repositoryUri: '210987654321.dkr.ecr.eu-central-1.amazonaws.com/aura-historia-second-image' } } },
  { name: 'wrong URI region', options: { repository: { repositoryUri: '123456789012.dkr.ecr.us-east-1.amazonaws.com/aura-historia-second-image' } } },
  { name: 'wrong URI repository', options: { repository: { repositoryUri: '123456789012.dkr.ecr.eu-central-1.amazonaws.com/aura-historia-other' } } },
  ...['MUTABLE', 'MUTABLE_WITH_EXCLUSION', 'IMMUTABLE_WITH_EXCLUSION', null].map((imageTagMutability) => ({
    name: `tag mutability ${imageTagMutability}`, options: { repository: { imageTagMutability } },
  })),
  { name: 'tag mutability exclusions', options: { repository: { imageTagMutabilityExclusionFilters: [{ filterType: 'WILDCARD', filter: 'git-*' }] } } },
  ...['KMS', 'KMS_DSSE', null].map((encryptionType) => ({
    name: `encryption ${encryptionType}`, options: { repository: { encryptionConfiguration: { encryptionType } } },
  })),
  { name: 'missing encryption configuration', options: { repository: { encryptionConfiguration: null } } },
]) {
  test(`repository validation rejects ${name} before any publication or repository mutation`, (t) => {
    const f = fixture(t, options);
    const result = f.publish();
    assert.notEqual(result.status, 0);
    assert.match(result.stderr, /Invalid ECR repository configuration: expected exactly one repository at 123456789012\.dkr\.ecr\.eu-central-1\.amazonaws\.com\/aura-historia-second-image/);
    assert.match(result.stderr, /IMMUTABLE without exclusions and AES256 encryption/);
    assert.doesNotMatch(result.stderr, /Cannot read ECR repository/);
    assertRepositoryValidationOnly(f);
  });
}

test('a second catalog image builds, publishes, reuses, resolves, and reaches the real deploy script', (t) => {
  const f = fixture(t);
  const build = workflowStep('container-images.yml', 'Build local image');
  f.run(process.execPath, ['ci/container-images.cjs', 'validate']);
  const matrix = JSON.parse(f.run(process.execPath, ['ci/container-images.cjs', 'matrix']).trim().slice('matrix='.length)).include;
  assert.deepEqual(matrix, f.entries);
  for (const entry of matrix) {
    f.run('bash', ['-e', '-c', build], f.imageEnv(entry));
    const result = f.publish(entry);
    assert.equal(result.status, 0, result.stderr);
  }
  const pushes = () => f.events().filter((event) => event.tool === 'docker' && event.args[0] === 'push');
  assert.equal(pushes().length, matrix.length);
  for (const entry of matrix) assert.ok(pushes().some((event) => event.args[1].endsWith(`/${entry.repository}:git-${sha}`)));
  assert.deepEqual(f.events().filter((event) => event.tool === 'smoke').map((event) => event.args), matrix.flatMap((entry) => [
    [entry.id, `local/${entry.id}:${sha}`], [entry.id, `local/${entry.id}:${sha}-123-1`],
  ]));

  const beforeReuse = f.events().length;
  for (const entry of matrix) {
    const result = f.publish(entry);
    assert.equal(result.status, 0, result.stderr);
  }
  const reused = f.events().slice(beforeReuse);
  assert.ok(!reused.some((event) => event.tool === 'smoke' || (event.tool === 'docker' && ['build', 'tag', 'push'].includes(event.args[0]))));
  assert.equal(reused.filter((event) => event.tool === 'docker' && event.args[0] === 'pull').length, matrix.length);
  assert.equal(reused.filter((event) => event.tool === 'docker' && event.args[0] === 'image').length, matrix.length);

  f.run(process.execPath, ['.github/actions/resolve-container-images/resolve.mjs']);
  const digests = JSON.parse(f.read('github-output').trim().slice('digests='.length));
  assert.deepEqual(digests, f.digests);
  const result = f.deploy({ CONTAINER_IMAGE_DIGESTS: JSON.stringify(digests) });
  assert.equal(result.status, 0, result.stderr);
  const deployments = assertDeployments(f, ['network', 'data', 'initialize', 'compute', 'api']);
  const compute = deployments.find((event) => event.args[6] === 'application-dev-compute');
  const parameters = compute.args.flatMap((arg, index) => arg === '--parameters' ? [compute.args[index + 1]] : []);
  assert.deepEqual(parameters.sort(), Object.entries({ CommitSHA: sha, ...digests }).map(([key, value]) => `application-dev-compute:${key}=${value}`).sort());
  for (const entry of matrix) {
    assert.ok(f.read('summary').includes(`- Registry digest: ${digests[entry.digestParameter]}`));
    assert.ok(f.read('summary').includes(`${entry.id}: arn:aws:ecs:eu-central-1:123456789012:task-definition/${entry.id}:1 (${entry.taskDefinitionOutput})`));
  }
});

test('a competing immutable publisher is verified and smoke-tested without retrying the push', (t) => {
  const f = fixture(t, { pushFailure: 'competing' });
  const result = f.publish();
  assert.equal(result.status, 0, result.stderr);
  assert.equal(f.events().filter((event) => event.tool === 'docker' && event.args[0] === 'push').length, 1);
  assert.deepEqual(f.events().filter((event) => event.tool === 'smoke').map((event) => event.args[1]), [
    `local/second-image:${sha}-123-1`, `123456789012.dkr.ecr.eu-central-1.amazonaws.com/aura-historia-second-image@${secondDigest}`,
  ]);
  assert.match(f.read('summary'), /Build workflow identity: https:\/\/github.com\/org\/repo\/actions\/runs\/456\/attempts\/2/);
});

for (const options of [{ smokeFailure: 'remote' }, { badMetadata: 'com.aura-historia.build-workflow' }]) {
  test(`a competing publication must pass ${options.smokeFailure ? 'its own smoke test' : 'provenance verification'}`, (t) => {
    const f = fixture(t, { pushFailure: 'competing', ...options });
    const result = f.publish();
    assert.notEqual(result.status, 0);
    assert.match(result.stderr, /smoke test failed|Image metadata does not match/);
    assert.equal(f.events().filter((event) => event.tool === 'docker' && event.args[0] === 'push').length, 1);
    assert.equal(f.read('summary'), '');
  });
}

test('a failed push with no trusted registry image stops without retrying or claiming success', (t) => {
  const f = fixture(t, { pushFailure: 'missing' });
  const result = f.publish();
  assert.notEqual(result.status, 0);
  assert.match(result.stderr, /No trusted registry image/);
  assert.equal(f.events().filter((event) => event.tool === 'docker' && event.args[0] === 'push').length, 1);
  assert.equal(f.read('summary'), '');
});

for (const ecrFailure of ['AccessDeniedException', 'RepositoryNotFoundException']) {
  test(`publication never treats ${ecrFailure} as a missing image`, (t) => {
    const f = fixture(t, { ecrFailure });
    const result = f.publish();
    assert.notEqual(result.status, 0);
    assert.ok(result.stderr.includes(ecrFailure));
    assert.ok(!f.events().some((event) => event.tool === 'docker' && ['build', 'tag', 'push'].includes(event.args[0])));
    assert.equal(f.read('summary'), '');
  });
}

for (const ecrResponse of ['invalid-digest', 'wrong-tag']) {
  test(`publication rejects ${ecrResponse} rather than replacing an immutable image`, (t) => {
    const f = fixture(t, { existingImages: true, ecrResponse });
    const result = f.publish();
    assert.notEqual(result.status, 0);
    assert.match(result.stderr, /invalid digest or tag association/);
    assert.ok(!f.events().some((event) => event.tool === 'docker' && event.args[0] !== 'login'));
    assert.equal(f.read('summary'), '');
  });
}

for (const badMetadata of ['Os', 'Architecture', 'org.opencontainers.image.revision', 'org.opencontainers.image.source', 'org.opencontainers.image.title', 'com.aura-historia.build-workflow']) {
  test(`reusing an image verifies ${badMetadata} without overwriting it`, (t) => {
    const f = fixture(t, { existingImages: true, badMetadata });
    const result = f.publish();
    assert.notEqual(result.status, 0);
    assert.match(result.stderr, /Image metadata does not match/);
    assert.ok(!f.events().some((event) => event.tool === 'docker' && ['build', 'tag', 'push'].includes(event.args[0])));
    assert.equal(f.read('summary'), '');
  });
}

for (const options of [{ smokeFailure: true }, { badMetadata: 'org.opencontainers.image.revision', badMetadataPhase: 'local' }]) {
  test(`new images must pass ${options.smokeFailure ? 'smoke tests' : 'metadata verification'} before publication`, (t) => {
    const f = fixture(t, options);
    const result = f.publish();
    assert.notEqual(result.status, 0);
    assert.match(result.stderr, /smoke test failed|Image metadata does not match/);
    assert.ok(!f.events().some((event) => event.tool === 'docker' && ['tag', 'push'].includes(event.args[0])));
    assert.equal(f.read('summary'), '');
  });
}

const absentStacks = ['network', 'data', 'initialize', 'compute', 'api'];
for (const scenario of [
  { name: 'first-time auto scope deploys foundation only', options: { absentStacks }, env: {}, expected: ['network', 'data', 'initialize'] },
  { name: 'explicit all scope admits first-time compute and API', options: { absentStacks }, env: { DEPLOY_SCOPE: 'all' }, expected: ['network', 'data', 'initialize', 'compute', 'api'] },
  { name: 'foundation scope skips compute and API without comparing migration sources', options: { gitDiffExitCode: 128 }, env: { DEPLOY_SCOPE: 'foundation' }, expected: ['network', 'data', 'initialize'] },
  { name: 'production application deployment includes observability', options: {}, env: { STAGE: 'prod' }, expected: ['network', 'data', 'initialize', 'compute', 'api', 'observability'] },
]) {
  test(scenario.name, (t) => {
    const f = fixture(t, scenario.options);
    const result = f.deploy(scenario.env);
    assert.equal(result.status, 0, result.stderr);
    assertDeployments(f, scenario.expected, scenario.env.STAGE ?? 'dev');
    assert.match(f.read('summary'), scenario.expected.includes('compute') ? /Application deployed/ : /Foundation deployed — application not deployed/);
    const events = f.events();
    const firstMutation = events.findIndex((event) => event.tool === 'ensure-dms-vpc-role');
    const preflight = events.slice(0, firstMutation).filter((event) => event.tool === 'aws');
    assert.equal(preflight.length, scenario.env.STAGE === 'prod' ? 6 : 5, 'All stacks must be preflighted before mutation');
    const comparesSources = (scenario.env.DEPLOY_SCOPE ?? 'auto') === 'auto' && !scenario.options.absentStacks?.includes('compute');
    assert.equal(events.filter((event) => event.tool === 'git').length, comparesSources ? 1 : 0);
    if (!scenario.expected.includes('compute')) assert.doesNotMatch(f.read('summary'), /task-definition\//);
  });
}

for (const gitDiffExitCode of [0, 1]) {
  test(`auto scope ${gitDiffExitCode === 0 ? 'updates the application when migration sources are unchanged' : 'deploys only foundation when migration sources changed'}`, (t) => {
    const currentSha = 'd'.repeat(40);
    const f = fixture(t, { computeCommitSha: currentSha, gitDiffExitCode });
    const result = f.deploy();
    assert.equal(result.status, 0, result.stderr);
    assertDeployments(f, gitDiffExitCode === 0 ? ['network', 'data', 'initialize', 'compute', 'api'] : ['network', 'data', 'initialize']);
    const events = f.events();
    assert.deepEqual(events.filter((event) => event.tool === 'git').map((event) => event.args), [[
      '--no-pager', 'diff', '--quiet', currentSha, sha,
      '--', 'migrations', 'infra/sql', 'src/database-migration-lambda',
    ]]);
    const diffIndex = events.findIndex((event) => event.tool === 'git');
    assert.equal(diffIndex, 5, 'All stacks must be inspected before comparing migration sources');
    assert.ok(diffIndex < events.findIndex((event) => event.tool === 'ensure-dms-vpc-role'));
    if (gitDiffExitCode === 1) {
      assert.match(f.read('summary'), /Foundation deployed — application not deployed/);
      assert.match(f.read('summary'), /Migration sources changed: true/);
      assert.match(f.read('summary'), /scope=all/);
      assert.doesNotMatch(f.read('summary'), /task-definition\//);
    } else {
      assert.match(f.read('summary'), /Application deployed/);
    }
  });
}

test('explicit all scope acknowledges migration readiness without comparing sources', (t) => {
  const f = fixture(t, { gitDiffExitCode: 1 });
  const result = f.deploy({ DEPLOY_SCOPE: 'all' });
  assert.equal(result.status, 0, result.stderr);
  assertDeployments(f, ['network', 'data', 'initialize', 'compute', 'api']);
  assert.equal(f.events().filter((event) => event.tool === 'git').length, 0);
  assert.match(f.read('summary'), /Application deployed/);
});

test('a Git lookup failure halts auto deployment before any mutation', (t) => {
  const f = fixture(t, { gitDiffExitCode: 128 });
  const result = f.deploy();
  assert.notEqual(result.status, 0);
  assert.match(result.stderr, /Cannot compare migration sources; no stacks were changed/);
  assertNoMutation(f, 1);
});

for (const computeCommitSha of [null, '', 'a'.repeat(39), 'A'.repeat(40), 'g'.repeat(40)]) {
  test(`auto scope rejects ${computeCommitSha === null ? 'a missing' : `invalid ${JSON.stringify(computeCommitSha)}`} compute CommitSHA before Git or mutations`, (t) => {
    const f = fixture(t, { computeCommitSha });
    const result = f.deploy();
    assert.notEqual(result.status, 0);
    if (computeCommitSha !== null) assert.match(result.stderr, /malformed CommitSHA/);
    assertNoMutation(f);
  });
}

test('stack permission errors halt before the DMS role or any deployment is changed', (t) => {
  const f = fixture(t, { deniedStack: 'api' });
  const result = f.deploy();
  assert.notEqual(result.status, 0);
  assert.match(result.stderr, /AccessDeniedException/);
  assertNoMutation(f);
});

for (const status of ['CREATE_FAILED', 'UPDATE_FAILED', 'ROLLBACK_COMPLETE', 'UPDATE_ROLLBACK_FAILED', 'CREATE_IN_PROGRESS', 'UPDATE_IN_PROGRESS', 'REVIEW_IN_PROGRESS', 'DELETE_IN_PROGRESS']) {
  test(`${status} stacks halt before any mutation, including foundation-only deployment`, (t) => {
    const f = fixture(t, { stackStatuses: { compute: status } });
    const result = f.deploy({ DEPLOY_SCOPE: 'foundation' });
    assert.notEqual(result.status, 0);
    assert.ok(result.stderr.includes(`compute is ${status}; repair it before deployment.`));
    assertNoMutation(f);
  });
}

test('production observability is checked before mutating foundation', (t) => {
  const f = fixture(t, { stackStatuses: { observability: 'UPDATE_IN_PROGRESS' } });
  const result = f.deploy({ STAGE: 'prod' });
  assert.notEqual(result.status, 0);
  assert.match(result.stderr, /observability is UPDATE_IN_PROGRESS/);
  assertNoMutation(f);
});

test('an incomplete catalog digest set halts before any mutation', (t) => {
  const f = fixture(t);
  const result = f.deploy({ CONTAINER_IMAGE_DIGESTS: JSON.stringify({ [f.entries[0].digestParameter]: firstDigest }) });
  assert.notEqual(result.status, 0);
  assert.match(result.stderr, /Resolved digest map must contain exactly/);
  assertNoMutation(f);
});

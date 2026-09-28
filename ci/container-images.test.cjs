'use strict';

const test = require('node:test');
const assert = require('node:assert/strict');
const { chmodSync, mkdtempSync, readFileSync, rmSync, writeFileSync } = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const { spawnSync } = require('node:child_process');
const { loadCatalog, resolveImage, resolveImages } = require('./container-images.cjs');

const sha = '6a9c07fa66163b0917434cbf6bcc20e40b9c845c';
const digest = `sha256:${'a'.repeat(64)}`;
const image = loadCatalog()[0];

function response(tag, value = digest) {
  return { imageDetails: [{ imageDigest: value, imageTags: [tag] }] };
}

test('the checked-in image catalog is valid', () => {
  assert.equal(loadCatalog().length, 1);
});

test('a missing image is the only response that permits a new build', () => {
  const missing = Object.assign(new Error('not found'), { code: 'ImageNotFoundException' });
  assert.equal(resolveImage(image, sha, () => { throw missing; }, { allowMissing: true }).status, 'missing');
  assert.throws(() => resolveImage(image, sha, () => { throw new Error('AccessDeniedException'); }, { allowMissing: true }), /AccessDeniedException/);
});

test('all catalog entries resolve by their own repository, SHA tag, and digest parameter', () => {
  const twoImages = [
    { ...image, id: 'one', repository: 'aura-one', digestParameter: 'OneDigest', taskDefinitionOutput: 'OneTaskArn' },
    { ...image, id: 'two', repository: 'aura-two', digestParameter: 'TwoDigest', taskDefinitionOutput: 'TwoTaskArn' },
  ];
  const resolved = resolveImages(twoImages, sha, (repository, tag) => response(tag, repository === 'aura-one' ? digest : `sha256:${'b'.repeat(64)}`));
  assert.deepEqual(Object.keys(resolved).sort(), ['OneDigest', 'TwoDigest']);
  assert.notEqual(resolved.OneDigest, resolved.TwoDigest);
});

test('the Initialize parameter selector handles absent and older compute stacks without activation toggles', () => {
  const directory = mkdtempSync(path.join(os.tmpdir(), 'initialize-parameters-'));
  try {
    const stackPath = path.join(directory, 'compute-stack.json');
    const digestsPath = path.join(directory, 'digests.json');
    const script = path.join(__dirname, 'container-images.cjs');
    const digests = { [image.digestParameter]: digest };
    writeFileSync(digestsPath, JSON.stringify(digests));
    const runSelector = () => spawnSync(process.execPath, [script, 'initialize-update', '--stack-file', stackPath, '--commit-sha', sha, '--digests', digestsPath], { encoding: 'utf8' });

    writeFileSync(stackPath, '');
    const absent = runSelector();
    assert.equal(absent.status, 0, absent.stderr);
    assert.deepEqual(JSON.parse(absent.stdout), {
      parameters: { CommitSHA: sha, [image.digestParameter]: digest },
      preserveExistingParameters: true,
    });

    writeFileSync(stackPath, JSON.stringify({ Stacks: [{ Parameters: [
      { ParameterKey: 'CommitSHA', ParameterValue: sha },
      { ParameterKey: 'PeriodicMatcherEnabled', ParameterValue: 'true' },
    ] }] }));
    const older = runSelector();
    assert.equal(older.status, 0, older.stderr);
    assert.deepEqual(JSON.parse(older.stdout).parameters, { CommitSHA: sha, [image.digestParameter]: digest });
    assert.equal(JSON.parse(older.stdout).parameters.PeriodicMatcherEnabled, undefined);

    writeFileSync(stackPath, JSON.stringify({ Stacks: [{ Parameters: [
      { ParameterKey: 'PeriodicMatcherImageDigest', ParameterValue: 'sha256:bad' },
    ] }] }));
    const malformed = runSelector();
    assert.notEqual(malformed.status, 0);
    assert.match(malformed.stderr, /malformed/);
  } finally {
    rmSync(directory, { recursive: true, force: true });
  }
});

test('the shared resolver action writes the digest-parameter map using a stub AWS CLI', () => {
  const directory = mkdtempSync(path.join(os.tmpdir(), 'container-resolver-'));
  try {
    const outputPath = path.join(directory, 'github-output');
    const awsPath = path.join(directory, 'aws');
    const workspace = path.resolve(__dirname, '..');
    const resolver = path.join(workspace, '.github/actions/resolve-container-images/resolve.mjs');
    writeFileSync(outputPath, '', { mode: 0o600 });
    writeFileSync(awsPath, `#!/usr/bin/env node\nconst tagArg = process.argv.find((arg) => arg.startsWith('imageTag='));\nif (!tagArg) process.exit(2);\nprocess.stdout.write(JSON.stringify({ imageDetails: [{ imageDigest: '${digest}', imageTags: [tagArg.slice('imageTag='.length)] }] }));\n`);
    chmodSync(awsPath, 0o755);
    const result = spawnSync(process.execPath, [resolver], {
      cwd: workspace,
      encoding: 'utf8',
      env: {
        ...process.env,
        PATH: `${directory}${path.delimiter}${process.env.PATH}`,
        GITHUB_WORKSPACE: workspace,
        GITHUB_OUTPUT: outputPath,
        CONTAINER_COMMIT_SHA: sha,
      },
    });
    assert.equal(result.status, 0, result.stderr);
    assert.equal(readFileSync(outputPath, 'utf8'), `digests=${JSON.stringify({ [image.digestParameter]: digest })}\n`);
  } finally {
    rmSync(directory, { recursive: true, force: true });
  }
});

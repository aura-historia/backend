'use strict';
const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const os = require('node:os');
const { publishAssets } = require('./release/assets.cjs');
const { ctx } = require('./test-support/release.cjs');
function assembly(t, mutate = () => {}) {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'release-assets-'));
  t.after(() => fs.rmSync(dir, { recursive: true, force: true }));
  fs.mkdirSync(path.join(dir, 'asset.hash'));
  fs.writeFileSync(path.join(dir, 'asset.hash', 'ca.pem'), 'fixture');
  const files = {
    tls: {
      source: { path: 'asset.hash', packaging: 'zip' },
      destinations: { current: { bucketName: 'staging', objectKey: 'dev/hash.zip' } },
    },
  };
  mutate(files);
  fs.writeFileSync(
    path.join(dir, 'manifest.json'),
    JSON.stringify({
      artifacts: {
        stack: { type: 'aws:cloudformation:stack', environment: 'aws://unknown-account/unknown-region' },
      },
    }),
  );
  fs.writeFileSync(path.join(dir, 'stack.assets.json'), JSON.stringify({ files }));
  return dir;
}
test('publishes environment-agnostic CDK ZIP assets once before deployment and reuses immutable objects', (t) => {
  const dir = assembly(t),
    calls = [];
  let exists = false;
  const run = (program, args) => {
    assert.equal(program, 'bash');
    if (args[0] === 'ci/s3-artifact-exists.sh') return exists ? 'true\n' : 'false\n';
    assert.equal(args[0], '-c');
    fs.writeFileSync(args.at(-1), 'zipped-fixture');
    return '';
  };
  const api = (_ctx, service, operation, args) => {
    assert.equal(service, 's3api');
    assert.equal(operation, 'put-object');
    assert.equal(args[args.indexOf('--expected-bucket-owner') + 1], ctx.account);
    assert.equal(args[args.indexOf('--if-none-match') + 1], '*');
    assert.equal(fs.readFileSync(args[args.indexOf('--body') + 1], 'utf8'), 'zipped-fixture');
    calls.push(args);
    exists = true;
  };
  publishAssets(ctx, dir, api, run);
  publishAssets(ctx, dir, api, run);
  assert.equal(calls.length, 1);
});
for (const [label, mutate] of [
  [
    'foreign region',
    (files) => {
      files.tls.destinations.current.region = 'us-east-1';
    },
  ],
  [
    'foreign role',
    (files) => {
      files.tls.destinations.current.assumeRoleArn = 'another-role';
    },
  ],
  [
    'escaping source',
    (files) => {
      files.tls.source.path = '../outside';
    },
  ],
])
  test(`rejects ${label} before asset publication`, (t) => {
    const dir = assembly(t, mutate);
    assert.throws(
      () =>
        publishAssets(
          ctx,
          dir,
          () => assert.fail(),
          () => assert.fail(),
        ),
      /unexpected|escapes/,
    );
  });
test('foreign stack environment cannot be hidden in an otherwise local assembly', (t) => {
  const dir = assembly(t);
  fs.writeFileSync(
    path.join(dir, 'manifest.json'),
    JSON.stringify({
      artifacts: {
        stack: { type: 'aws:cloudformation:stack', environment: 'aws://999999999999/eu-central-1' },
      },
    }),
  );
  assert.throws(
    () =>
      publishAssets(
        ctx,
        dir,
        () => assert.fail(),
        () => assert.fail(),
      ),
    /another account/,
  );
});

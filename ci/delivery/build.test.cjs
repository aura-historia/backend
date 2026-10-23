'use strict';
const test = require('node:test'),
  assert = require('node:assert/strict');
const {
  completeBundle,
  validateObjects,
  loadBuildConfig,
} = require('./build.cjs');
const { bytesDigest } = require('./contracts.cjs');
const f = require('./fixtures.cjs');
function publication() {
  const bundle = f.bundle(),
    body = Buffer.from('artifact');
  for (const asset of bundle.artifacts) {
    asset.sha256 = bytesDigest(body);
    asset.key = `objects/sha256/${asset.sha256}`;
    asset.size = body.length;
  }
  bundle.targets.prod.assemblyDigest = bundle.artifacts[1].sha256;
  const writes = [],
    objects = new Map();
  const store = {
    readObject: () => ({ body }),
    putImmutableJson: (_bucket, key, value) => {
      const serialized = JSON.stringify(value);
      if (objects.has(key) && objects.get(key) !== serialized)
        throw Error('conflict');
      objects.set(key, serialized);
      writes.push(key);
    },
  };
  return { bundle, body, writes, objects, store };
}
test('manifest and completion marker publish last after all object verification; retries reuse bytes', () => {
  const p = publication(),
    first = completeBundle(p.bundle, 'bundles', p.store);
  assert.match(p.writes[0], /^bundles\/[a-f0-9]{64}\.json$/);
  assert.match(p.writes[1], /^builds\/.+\/complete\.json$/);
  assert.equal(completeBundle(p.bundle, 'bundles', p.store), first);
});
test('missing corrupted or incomplete artifact never publishes a completion marker', () => {
  for (const failure of ['missing', 'corrupt', 'catalog']) {
    const p = publication();
    if (failure === 'missing')
      p.store.readObject = () => {
        throw Error('missing');
      };
    if (failure === 'corrupt')
      p.store.readObject = () => ({ body: Buffer.from('altered') });
    if (failure === 'catalog') p.bundle.artifacts.shift();
    assert.throws(() => completeBundle(p.bundle, 'bundles', p.store));
    assert.deepEqual(p.writes, []);
  }
});
test('late manifest publication failure cannot write a completion marker', () => {
  const p = publication();
  p.store.putImmutableJson = () => {
    throw Error('interrupted upload');
  };
  assert.throws(() => completeBundle(p.bundle, 'bundles', p.store));
  assert.deepEqual(p.writes, []);
});
test('build tooling identity is pinned and production works without dev', () => {
  const config = loadBuildConfig(undefined, { AWS_ACCOUNT_ID: '123456789012' });
  assert.deepEqual(Object.keys(config.targets), ['prod']);
  const enabled = loadBuildConfig(undefined, {
    AWS_ACCOUNT_ID: '123456789012',
    DELIVERY_DEV_ENABLED: 'true',
  });
  assert.ok(enabled.targets.dev);
  assert.notDeepEqual(config, enabled);
  assert.throws(() => loadBuildConfig(undefined, { AWS_ACCOUNT_ID: 'wrong' }));
});

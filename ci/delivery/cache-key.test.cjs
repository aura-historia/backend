'use strict';
const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const { cacheIdentity } = require('./cache-key.cjs');

test('dependency snapshots survive application edits but separate incompatible build inputs', () => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), 'release-cache-test-'));
  try {
    fs.mkdirSync(path.join(root, 'ci'));
    fs.writeFileSync(path.join(root, 'Cargo.lock'), 'locked dependencies');
    fs.writeFileSync(path.join(root, 'ci/lambda-binaries.json'), '["api"]');
    const config = { toolchain: '1.98.0', builderImage: 'pinned-builder', profile: 'release' };
    const initial = cacheIdentity(root, config);
    fs.writeFileSync(path.join(root, 'main.rs'), 'application changed');
    assert.equal(cacheIdentity(root, config), initial);
    for (const changed of [
      { toolchain: 'another compiler' }, { builderImage: 'another builder' },
      { profile: 'another optimization policy' },
    ]) assert.notEqual(cacheIdentity(root, { ...config, ...changed }), initial);
    fs.writeFileSync(path.join(root, 'Cargo.lock'), 'updated dependencies');
    assert.notEqual(cacheIdentity(root, config), initial);
    fs.writeFileSync(path.join(root, 'Cargo.lock'), 'locked dependencies');
    fs.writeFileSync(path.join(root, 'ci/lambda-binaries.json'), '["api","worker"]');
    assert.notEqual(cacheIdentity(root, config), initial);
  } finally {
    fs.rmSync(root, { recursive: true, force: true });
  }
});

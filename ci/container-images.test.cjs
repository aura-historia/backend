'use strict';

const test = require('node:test');
const assert = require('node:assert/strict');
const { loadCatalog, validateCatalog } = require('./container-images.cjs');

const image = loadCatalog()[0];

test('the checked-in image catalog is valid', () => {
  assert.equal(loadCatalog().length, 1);
});

test('two images may not share an id or a GHCR package', () => {
  assert.throws(() => validateCatalog([image, { ...image }]), /duplicate id/);
  assert.throws(() => validateCatalog([image, { ...image, id: 'other' }]), /duplicate repository/);
});

test('an image needs a declared binary and its own smoke test', () => {
  assert.throws(() => validateCatalog([{ ...image, binary: 'not-a-binary' }]), /is not declared/);
  assert.throws(() => validateCatalog([{ ...image, id: 'unknown' }]), /smoke test/);
});

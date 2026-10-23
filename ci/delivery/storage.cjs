'use strict';
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const { execFileSync } = require('node:child_process');
const { canonicalize, bytesDigest } = require('./contracts.cjs');
function aws(service, operation, args = []) {
  return JSON.parse(
    execFileSync('aws', [service, operation, ...args, '--output', 'json'], {
      encoding: 'utf8',
      stdio: ['ignore', 'pipe', 'inherit'],
      maxBuffer: 32 * 1024 * 1024,
    }) || '{}',
  );
}
function readObject(bucket, key, versionId) {
  const temporary = fs.mkdtempSync(path.join(os.tmpdir(), 'delivery-read-'));
  const file = path.join(temporary, 'object');
  try {
    const metadata = aws('s3api', 'get-object', [
      '--bucket',
      bucket,
      '--key',
      key,
      ...(versionId ? ['--version-id', versionId] : []),
      file,
    ]);
    return { body: fs.readFileSync(file), versionId: metadata.VersionId };
  } finally {
    fs.rmSync(temporary, { recursive: true, force: true });
  }
}
function readJson(bucket, key, versionId) {
  return JSON.parse(readObject(bucket, key, versionId).body);
}
function immutablePut(bucket, key, file) {
  if (
    !fs.lstatSync(file).isFile() ||
    fs.realpathSync(file) !== path.resolve(file)
  )
    throw new Error('Publication requires a regular file');
  const body = fs.readFileSync(file),
    sha256 = bytesDigest(body);
  try {
    const metadata = aws('s3api', 'put-object', [
      '--bucket',
      bucket,
      '--key',
      key,
      '--body',
      file,
      '--if-none-match',
      '*',
      '--checksum-algorithm',
      'SHA256',
      '--metadata',
      `sha256=${sha256}`,
    ]);
    if (!metadata.VersionId || metadata.VersionId === 'null')
      throw new Error('Delivery bucket must have versioning enabled');
    return {
      bucket,
      key,
      versionId: metadata.VersionId,
      sha256,
      size: body.length,
    };
  } catch (cause) {
    // Failed uploads and concurrent writers reconcile only byte-identical existing objects.
    // A denied/missing object propagates the real AWS failure; never overwrite it.
    let existing;
    try {
      existing = readObject(bucket, key);
    } catch {
      throw cause;
    }
    if (
      bytesDigest(existing.body) !== sha256 ||
      !existing.versionId ||
      existing.versionId === 'null'
    )
      throw new Error(`Immutable publication conflict: ${key}`, { cause });
    return {
      bucket,
      key,
      versionId: existing.versionId,
      sha256,
      size: body.length,
    };
  }
}
function publishFile(bucket, file, kind, id) {
  if (
    !fs.lstatSync(file).isFile() ||
    fs.realpathSync(file) !== path.resolve(file)
  )
    throw new Error('Publication requires a regular file');
  const sha256 = bytesDigest(fs.readFileSync(file));
  return {
    id,
    kind,
    ...immutablePut(bucket, `objects/sha256/${sha256}`, file),
  };
}
function putImmutableJson(bucket, key, value) {
  const temporary = fs.mkdtempSync(path.join(os.tmpdir(), 'delivery-json-'));
  try {
    const file = path.join(temporary, 'document.json');
    fs.writeFileSync(file, canonicalize(value));
    return immutablePut(bucket, key, file);
  } finally {
    fs.rmSync(temporary, { recursive: true, force: true });
  }
}
module.exports = {
  aws,
  readObject,
  readJson,
  immutablePut,
  publishFile,
  putImmutableJson,
};

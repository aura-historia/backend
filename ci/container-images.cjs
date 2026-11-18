#!/usr/bin/env node
'use strict';

const fs = require('node:fs');
const path = require('node:path');

const ALLOWED_PLATFORMS = new Set(['linux/amd64', 'linux/arm64']);

function requireString(value, label, pattern) {
  if (typeof value !== 'string' || value.length === 0 || (pattern && !pattern.test(value))) {
    throw new Error(`Container image catalog has an invalid ${label}.`);
  }
}

function checkedPath(rootDir, relativePath, label) {
  requireString(relativePath, label);
  if (path.isAbsolute(relativePath) || relativePath.includes('\\') || relativePath.split('/').some((part) => part === '' || part === '.' || part === '..')) {
    throw new Error(`Container image catalog ${label} must be a safe repository-relative path.`);
  }
  const absolutePath = path.resolve(rootDir, relativePath);
  if (absolutePath !== rootDir && !absolutePath.startsWith(`${rootDir}${path.sep}`)) {
    throw new Error(`Container image catalog ${label} escapes the repository root.`);
  }
  return absolutePath;
}

function manifestSection(manifest, header) {
  const start = manifest.match(new RegExp(`^\\[${header}\\]\\s*$`, 'm'));
  if (!start || start.index === undefined) return '';
  const contentStart = start.index + start[0].length;
  const next = manifest.slice(contentStart).search(/^\[/m);
  return manifest.slice(contentStart, next === -1 ? undefined : contentStart + next);
}

function binaryExists(crateDir, binary) {
  const manifestPath = path.join(crateDir, 'Cargo.toml');
  if (!fs.existsSync(manifestPath)) return false;
  const manifest = fs.readFileSync(manifestPath, 'utf8');
  const packageName = manifestSection(manifest, 'package').match(/^name\s*=\s*"([^"]+)"/m)?.[1];
  if (packageName === binary && fs.existsSync(path.join(crateDir, 'src', 'main.rs'))) return true;
  if (fs.existsSync(path.join(crateDir, 'src', 'bin', `${binary}.rs`))) return true;
  if (fs.existsSync(path.join(crateDir, 'src', 'bin', binary, 'main.rs'))) return true;
  const declaredBins = [...manifest.matchAll(/^\[\[bin\]\]([\s\S]*?)(?=^\[|$)/gm)];
  return declaredBins.some((match) => match[1].match(/^name\s*=\s*"([^"]+)"/m)?.[1] === binary);
}

function validateCatalog(catalog, rootDir = path.resolve(__dirname, '..')) {
  if (!Array.isArray(catalog) || catalog.length === 0) {
    throw new Error('Container image catalog must be a non-empty array.');
  }
  const keys = ['id', 'repository'];
  const seen = Object.fromEntries(keys.map((key) => [key, new Set()]));
  const validated = catalog.map((entry, index) => {
    if (!entry || typeof entry !== 'object' || Array.isArray(entry)) {
      throw new Error(`Container image catalog entry ${index} must be an object.`);
    }
    requireString(entry.id, `entry ${index} id`, /^[a-z][a-z0-9-]*$/);
    requireString(entry.crate, `${entry.id} crate`);
    requireString(entry.binary, `${entry.id} binary`, /^[a-z0-9]+(?:-[a-z0-9]+)*$/);
    requireString(entry.dockerfile, `${entry.id} Dockerfile`);
    requireString(entry.repository, `${entry.id} GHCR package`, /^[a-z0-9]+(?:[._-][a-z0-9]+)*$/);
    if (!ALLOWED_PLATFORMS.has(entry.platform)) {
      throw new Error(`Container image '${entry.id}' uses unsupported platform '${entry.platform}'.`);
    }
    for (const key of keys) {
      if (seen[key].has(entry[key])) throw new Error(`Container image catalog has duplicate ${key} '${entry[key]}'.`);
      seen[key].add(entry[key]);
    }

    const crateDir = checkedPath(rootDir, entry.crate, `${entry.id} crate path`);
    const dockerfilePath = checkedPath(rootDir, entry.dockerfile, `${entry.id} Dockerfile path`);
    const smokePath = checkedPath(rootDir, `ci/container-images/${entry.id}/smoke.sh`, `${entry.id} smoke-test path`);
    if (!fs.statSync(crateDir, { throwIfNoEntry: false })?.isDirectory()) {
      throw new Error(`Container image '${entry.id}' crate directory does not exist: ${entry.crate}.`);
    }
    if (!fs.statSync(dockerfilePath, { throwIfNoEntry: false })?.isFile()) {
      throw new Error(`Container image '${entry.id}' Dockerfile does not exist: ${entry.dockerfile}.`);
    }
    if (!binaryExists(crateDir, entry.binary)) {
      throw new Error(`Container image '${entry.id}' binary '${entry.binary}' is not declared by ${entry.crate}/Cargo.toml.`);
    }
    if (!fs.statSync(smokePath, { throwIfNoEntry: false })?.isFile()) {
      throw new Error(`Container image '${entry.id}' must own an image smoke test at ci/container-images/${entry.id}/smoke.sh.`);
    }
    return Object.freeze({ ...entry });
  });
  return Object.freeze(validated);
}

function loadCatalog(catalogPath = path.join(__dirname, 'container-images.json'), rootDir = path.resolve(__dirname, '..')) {
  let parsed;
  try {
    parsed = JSON.parse(fs.readFileSync(catalogPath, 'utf8'));
  } catch (error) {
    throw new Error(`Unable to read container image catalog ${catalogPath}: ${error.message}`);
  }
  return validateCatalog(parsed, rootDir);
}

function parseArgs(argv) {
  const options = {};
  for (let index = 0; index < argv.length; index += 1) {
    const arg = argv[index];
    if (arg.startsWith('--')) {
      const key = arg.slice(2);
      const value = argv[index + 1];
      if (!value || value.startsWith('--')) throw new Error(`Missing value for ${arg}.`);
      options[key] = value;
      index += 1;
    } else {
      throw new Error(`Unexpected argument '${arg}'.`);
    }
  }
  return options;
}

function main(argv = process.argv.slice(2)) {
  const [command, ...rest] = argv;
  const options = parseArgs(rest);
  const rootDir = path.resolve(__dirname, '..');
  const catalogPath = options.catalog ? path.resolve(options.catalog) : path.join(__dirname, 'container-images.json');
  const catalog = loadCatalog(catalogPath, rootDir);
  if (command === 'validate') {
    process.stdout.write(`Validated ${catalog.length} container image catalog entr${catalog.length === 1 ? 'y' : 'ies'}.\n`);
    return;
  }
  if (command === 'matrix') {
    process.stdout.write(`matrix=${JSON.stringify({ include: catalog })}\n`);
    return;
  }
  throw new Error('Usage: container-images.cjs validate|matrix [--catalog <file>].');
}

if (require.main === module) {
  try {
    main();
  } catch (error) {
    process.stderr.write(`${error.message}\n`);
    process.exitCode = 1;
  }
}

module.exports = { loadCatalog, validateCatalog };

#!/usr/bin/env node
'use strict';

const fs = require('node:fs');
const path = require('node:path');
const { execFileSync } = require('node:child_process');

const DIGEST_PATTERN = /^sha256:[0-9a-f]{64}$/;
const SHA_PATTERN = /^[0-9a-f]{40}$/;
const ALLOWED_PLATFORMS = new Set(['linux/amd64', 'linux/arm64']);
const NON_IMAGE_ACTIVATION_PARAMETERS = ['CdcRouterEnabled'];
const activationParameters = (catalog) => [...NON_IMAGE_ACTIVATION_PARAMETERS, ...catalog.map((image) => image.activationParameter).filter(Boolean)];

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
  const keys = ['id', 'repository', 'digestParameter', 'taskDefinitionOutput'];
  const seen = Object.fromEntries(keys.map((key) => [key, new Set()]));
  const ids = new Set();
  const activationNames = new Set(NON_IMAGE_ACTIVATION_PARAMETERS);
  const validated = catalog.map((entry, index) => {
    if (!entry || typeof entry !== 'object' || Array.isArray(entry)) {
      throw new Error(`Container image catalog entry ${index} must be an object.`);
    }
    requireString(entry.id, `entry ${index} id`, /^[a-z][a-z0-9-]*$/);
    requireString(entry.crate, `${entry.id} crate`);
    requireString(entry.binary, `${entry.id} binary`, /^[a-z0-9]+(?:-[a-z0-9]+)*$/);
    requireString(entry.dockerfile, `${entry.id} Dockerfile`);
    requireString(entry.repository, `${entry.id} ECR repository`, /^[a-z0-9]+(?:[._-][a-z0-9]+)*$/);
    requireString(entry.digestParameter, `${entry.id} digest parameter`, /^[A-Za-z][A-Za-z0-9]*$/);
    requireString(entry.taskDefinitionOutput, `${entry.id} task-definition output`, /^[A-Za-z][A-Za-z0-9]*$/);
    if (entry.activationParameter !== undefined) requireString(entry.activationParameter, `${entry.id} activation parameter`, /^[A-Za-z][A-Za-z0-9]*$/);
    if (!ALLOWED_PLATFORMS.has(entry.platform)) {
      throw new Error(`Container image '${entry.id}' uses unsupported platform '${entry.platform}'.`);
    }
    for (const key of keys) {
      if (seen[key].has(entry[key])) throw new Error(`Container image catalog has duplicate ${key} '${entry[key]}'.`);
      seen[key].add(entry[key]);
    }
    if (entry.activationParameter) {
      if (activationNames.has(entry.activationParameter) || seen.digestParameter.has(entry.activationParameter)) {
        throw new Error(`Container image catalog has duplicate activation parameter '${entry.activationParameter}'.`);
      }
      activationNames.add(entry.activationParameter);
    }
    if (activationNames.has(entry.digestParameter)) throw new Error(`Container image catalog digest parameter '${entry.digestParameter}' overlaps an activation parameter.`);
    if (ids.has(entry.id)) throw new Error(`Container image catalog has duplicate id '${entry.id}'.`);
    ids.add(entry.id);

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

function imageById(catalog, id) {
  const matches = catalog.filter((entry) => entry.id === id);
  if (matches.length !== 1) throw new Error(`Expected exactly one catalog entry for image '${id}'.`);
  return matches[0];
}

function isImageNotFound(error) {
  return error && error.code === 'ImageNotFoundException';
}

function validatedImageResult(result, image, tag) {
  if (!result || typeof result !== 'object' || !Array.isArray(result.imageDetails) || result.imageDetails.length !== 1) {
    throw new Error(`ECR returned a malformed or ambiguous result for ${image.repository}:${tag}.`);
  }
  const detail = result.imageDetails[0];
  if (!detail || !DIGEST_PATTERN.test(detail.imageDigest) || !Array.isArray(detail.imageTags) || !detail.imageTags.includes(tag)) {
    throw new Error(`ECR returned an invalid digest or tag association for ${image.repository}:${tag}.`);
  }
  return Object.freeze({ status: 'existing', repository: image.repository, tag, digest: detail.imageDigest });
}

function resolveImage(image, sha, describeImages, { allowMissing = false } = {}) {
  requireString(sha, 'source commit SHA', SHA_PATTERN);
  const tag = `git-${sha}`;
  let result;
  try {
    result = describeImages(image.repository, tag);
  } catch (error) {
    if (isImageNotFound(error) && allowMissing) return Object.freeze({ status: 'missing', repository: image.repository, tag });
    if (isImageNotFound(error)) throw new Error(`Missing immutable image ${image.repository}:${tag}.`);
    throw error;
  }
  return validatedImageResult(result, image, tag);
}

function awsDescribeImages(repository, tag, runCommand = execFileSync) {
  let stdout;
  try {
    stdout = runCommand('aws', [
      'ecr', 'describe-images', '--repository-name', repository,
      '--image-ids', `imageTag=${tag}`, '--output', 'json',
    ], { encoding: 'utf8', stdio: ['ignore', 'pipe', 'pipe'] });
  } catch (error) {
    const stderr = Buffer.isBuffer(error.stderr) ? error.stderr.toString('utf8') : String(error.stderr ?? '');
    const stdoutError = Buffer.isBuffer(error.stdout) ? error.stdout.toString('utf8') : String(error.stdout ?? '');
    const diagnostic = `${stderr}\n${stdoutError}\n${error.message ?? ''}`;
    if (/(?:^|\s|\()ImageNotFoundException(?:\)|\s|$)/.test(diagnostic)) {
      const notFound = new Error(`ECR image ${repository}:${tag} was not found.`);
      notFound.code = 'ImageNotFoundException';
      throw notFound;
    }
    throw new Error(`ECR lookup failed for ${repository}:${tag}: ${diagnostic.trim() || 'AWS CLI failed.'}`);
  }
  try {
    return JSON.parse(stdout);
  } catch (error) {
    throw new Error(`ECR returned malformed JSON for ${repository}:${tag}: ${error.message}`);
  }
}

function resolveImages(catalog, sha, describeImages = awsDescribeImages) {
  requireString(sha, 'source commit SHA', SHA_PATTERN);
  const digestByParameter = {};
  for (const image of catalog) {
    const resolved = resolveImage(image, sha, describeImages);
    digestByParameter[image.digestParameter] = resolved.digest;
  }
  return digestByParameter;
}

function validateDigestMap(digestByParameter, catalog) {
  if (!digestByParameter || typeof digestByParameter !== 'object' || Array.isArray(digestByParameter)) {
    throw new Error('Resolved image digest map must be a JSON object.');
  }
  const expected = catalog.map((image) => image.digestParameter).sort();
  const actual = Object.keys(digestByParameter).sort();
  if (JSON.stringify(actual) !== JSON.stringify(expected)) {
    throw new Error(`Resolved digest map must contain exactly: ${expected.join(', ')}.`);
  }
  for (const key of expected) {
    if (typeof digestByParameter[key] !== 'string' || !DIGEST_PATTERN.test(digestByParameter[key])) {
      throw new Error(`Resolved image digest for ${key} is invalid.`);
    }
  }
}

function parameterValues(parameters, key) {
  return parameters.filter((parameter) => parameter && parameter.ParameterKey === key);
}

function validateNewTemplateParameters(parameters, catalog) {
  if (!Array.isArray(parameters)) throw new Error('CloudFormation stack has no valid Parameters array.');
  const values = new Map();
  const activations = activationParameters(catalog);
  const keys = [...new Set([...catalog.map((image) => image.digestParameter), ...activations])];
  for (const key of keys) {
    const matches = parameterValues(parameters, key);
    if (matches.length > 1) throw new Error(`Compute stack has duplicate ${key} parameter entries.`);
    if (matches.length === 0) continue;
    const value = matches[0].ParameterValue;
    if (typeof value !== 'string' || value.length === 0) throw new Error(`Compute stack ${key} parameter is empty or malformed.`);
    if (catalog.some((image) => image.digestParameter === key) && !DIGEST_PATTERN.test(value)) {
      throw new Error(`Compute stack ${key} parameter is malformed; expected a sha256 digest.`);
    }
    if (activations.includes(key) && value !== 'true' && value !== 'false') {
      throw new Error(`Compute stack ${key} parameter is malformed; expected true or false.`);
    }
    values.set(key, value);
  }
  return values;
}

function prepareNewTemplateUpdate(stackDocument, sha, digestByParameter, catalog) {
  requireString(sha, 'source commit SHA', SHA_PATTERN);
  validateDigestMap(digestByParameter, catalog);
  let parameters = [];
  const stack = stackDocument?.Stacks?.[0];
  if (stackDocument !== null && stackDocument !== undefined) {
    if (!stack || !Array.isArray(stack.Parameters)) throw new Error('Existing compute stack has no valid Parameters array.');
    parameters = stack.Parameters;
    validateNewTemplateParameters(parameters, catalog);
    const commitValues = parameterValues(parameters, 'CommitSHA');
    if (commitValues.length > 1 || (commitValues.length === 1 && (typeof commitValues[0].ParameterValue !== 'string' || !SHA_PATTERN.test(commitValues[0].ParameterValue)))) {
      throw new Error('Existing compute stack has a duplicate or malformed CommitSHA parameter.');
    }
  }
  return Object.freeze({
    parameters: Object.freeze({ CommitSHA: sha, ...digestByParameter }),
    preserveExistingParameters: true,
  });
}

function preparePreviousTemplateUpdate(stackDocument, sha, digestByParameter, catalog) {
  requireString(sha, 'source commit SHA', SHA_PATTERN);
  validateDigestMap(digestByParameter, catalog);
  const stack = stackDocument?.Stacks?.[0];
  const parameters = stack?.Parameters;
  if (!Array.isArray(parameters)) throw new Error('Previous-template compute stack has no valid Parameters array.');
  const present = validateNewTemplateParameters(parameters, catalog);
  const commitSha = parameterValues(parameters, 'CommitSHA');
  if (commitSha.length !== 1 || typeof commitSha[0].ParameterValue !== 'string' || !SHA_PATTERN.test(commitSha[0].ParameterValue)) {
    throw new Error('Compute stack must have exactly one valid CommitSHA parameter.');
  }
  for (const key of activationParameters(catalog)) {
    if (!present.has(key)) throw new Error(`Unsupported historical compute template: missing ${key}; deploy an infrastructure-bearing release first.`);
  }
  for (const image of catalog) {
    if (!present.has(image.digestParameter)) {
      throw new Error(`Unsupported historical compute template: missing ${image.digestParameter}; deploy an infrastructure-bearing release first.`);
    }
  }
  const outputs = stack.Outputs;
  for (const image of catalog) {
    const matches = Array.isArray(outputs) ? outputs.filter((output) => output && output.OutputKey === image.taskDefinitionOutput) : [];
    if (matches.length !== 1 || !validTaskDefinitionArn(matches[0].OutputValue)) {
      throw new Error(`Unsupported historical compute template: missing or invalid ${image.taskDefinitionOutput}; deploy an infrastructure-bearing release first.`);
    }
  }
  const unchanged = commitSha[0].ParameterValue === sha && catalog.every((image) => present.get(image.digestParameter) === digestByParameter[image.digestParameter]);
  const updates = parameters.map((parameter) => {
    if (parameter.ParameterKey === 'CommitSHA') return { ParameterKey: 'CommitSHA', ParameterValue: sha };
    if (Object.prototype.hasOwnProperty.call(digestByParameter, parameter.ParameterKey)) {
      return { ParameterKey: parameter.ParameterKey, ParameterValue: digestByParameter[parameter.ParameterKey] };
    }
    return { ParameterKey: parameter.ParameterKey, UsePreviousValue: true };
  });
  return Object.freeze({ unchanged, parameters: updates });
}

function validTaskDefinitionArn(value) {
  return typeof value === 'string' && /^arn:[^:]+:ecs:[^:]+:\d{12}:task-definition\/[A-Za-z0-9_-]+:\d+$/.test(value);
}

function extractTaskDefinitionOutputs(stackDocument, catalog, stackName = 'compute stack') {
  const outputs = stackDocument?.Stacks?.[0]?.Outputs;
  if (!Array.isArray(outputs)) throw new Error(`Post-deployment output lookup failed for ${stackName}: Outputs is missing.`);
  return catalog.map((image) => {
    const matches = outputs.filter((output) => output && output.OutputKey === image.taskDefinitionOutput);
    if (matches.length !== 1 || typeof matches[0].OutputValue !== 'string' || matches[0].OutputValue.length === 0) {
      throw new Error(`Post-deployment output lookup failed for ${stackName}: expected exactly one non-empty ${image.taskDefinitionOutput} output. The deployment may already have completed; inspect the existing stack before rerunning, and do not rerun migrations or launch another task.`);
    }
    if (!validTaskDefinitionArn(matches[0].OutputValue)) {
      throw new Error(`Post-deployment output ${image.taskDefinitionOutput} from ${stackName} is not a valid ECS task-definition ARN. The deployment may already have completed; inspect the existing stack before rerunning.`);
    }
    return Object.freeze({ id: image.id, output: image.taskDefinitionOutput, value: matches[0].OutputValue });
  });
}

function parseArgs(argv) {
  const options = {};
  for (let index = 0; index < argv.length; index += 1) {
    const arg = argv[index];
    if (arg === '--allow-missing') options.allowMissing = true;
    else if (arg.startsWith('--')) {
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
  if (command === 'resolve-one') {
    const image = imageById(catalog, options.id);
    const result = resolveImage(image, options['commit-sha'], awsDescribeImages, { allowMissing: options.allowMissing });
    process.stdout.write(`${JSON.stringify(result)}\n`);
    return;
  }
  if (command === 'resolve-all') {
    process.stdout.write(`${JSON.stringify(resolveImages(catalog, options['commit-sha']))}\n`);
    return;
  }
  if (command === 'task-outputs') {
    const stackDocument = JSON.parse(fs.readFileSync(options['stack-file'], 'utf8'));
    const results = extractTaskDefinitionOutputs(stackDocument, catalog, options['stack-name']);
    for (const result of results) process.stdout.write(`- ${result.id}: ${result.value} (${result.output})\n`);
    return;
  }
  if (command === 'validate-new-template') {
    const stackDocument = JSON.parse(fs.readFileSync(options['stack-file'], 'utf8'));
    validateNewTemplateParameters(stackDocument?.Stacks?.[0]?.Parameters, catalog);
    return;
  }
  if (command === 'initialize-update') {
    const stackText = fs.readFileSync(options['stack-file'], 'utf8');
    const stackDocument = stackText.trim() ? JSON.parse(stackText) : null;
    const digests = JSON.parse(fs.readFileSync(options.digests, 'utf8'));
    process.stdout.write(`${JSON.stringify(prepareNewTemplateUpdate(stackDocument, options['commit-sha'], digests, catalog))}\n`);
    return;
  }
  if (command === 'previous-update') {
    const stackDocument = JSON.parse(fs.readFileSync(options['stack-file'], 'utf8'));
    const digests = JSON.parse(fs.readFileSync(options.digests, 'utf8'));
    process.stdout.write(`${JSON.stringify(preparePreviousTemplateUpdate(stackDocument, options['commit-sha'], digests, catalog))}\n`);
    return;
  }
  throw new Error('Usage: container-images.cjs validate|matrix|resolve-one|resolve-all|task-outputs|validate-new-template|initialize-update|previous-update [options].');
}

if (require.main === module) {
  try {
    main();
  } catch (error) {
    process.stderr.write(`${error.message}\n`);
    process.exitCode = 1;
  }
}

module.exports = {

  DIGEST_PATTERN,
  SHA_PATTERN,
  awsDescribeImages,
  extractTaskDefinitionOutputs,
  loadCatalog,
  prepareNewTemplateUpdate,
  preparePreviousTemplateUpdate,
  resolveImage,
  resolveImages,
  validateCatalog,
  validateDigestMap,
  validateNewTemplateParameters,
};

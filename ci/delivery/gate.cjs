'use strict';
const fs = require('node:fs');
const path = require('node:path');
const { execFileSync } = require('node:child_process');
const {
  digest,
  bytesDigest,
  validateTestEvidence,
} = require('./contracts.cjs');
const REQUIRED_JOBS = Object.freeze([
  'rust',
  'infra',
  'templates',
  'images',
  'inventory',
]);

function inventory(root) {
  const files = execFileSync('git', ['ls-files', '-z'], { cwd: root })
    .toString()
    .split('\0')
    .filter(Boolean);
  const tracked = files.filter(
    (file) =>
      /^(Cargo\.toml|Cargo\.lock|rust-toolchain\.toml|depgraph-rules\.toml|\.config\/|\.cargo\/|ci\/|tooling\/|tests\/|\.github\/|infra\/package(-lock)?\.json|mjml\/package(-lock)?\.json)/.test(
        file,
      ) || /^src\/.*\/(Cargo\.toml|Dockerfile)$/.test(file),
  );
  const lanes = JSON.parse(
    fs.readFileSync(path.join(root, 'ci/test-lanes.json')),
  );
  const metadata = JSON.parse(
    execFileSync(
      'cargo',
      ['metadata', '--locked', '--no-deps', '--format-version', '1'],
      { cwd: root, maxBuffer: 16 * 1024 * 1024 },
    ),
  );
  const packages = metadata.packages.filter((pkg) =>
    metadata.workspace_members.includes(pkg.id),
  );
  const assigned = Object.values(lanes).flat();
  if (
    new Set(assigned).size !== assigned.length ||
    digest([...assigned].sort()) !==
      digest(packages.map((pkg) => pkg.name).sort())
  )
    throw new Error('Incomplete or duplicate workspace test catalog');
  const images = JSON.parse(
    fs.readFileSync(path.join(root, 'ci/container-images.json')),
  );
  const lambdas = JSON.parse(
    fs.readFileSync(path.join(root, 'ci/lambda-binaries.json')),
  );
  const imageBinaries = images.map((image) => image.binary);
  const expected = packages.flatMap((pkg) =>
    pkg.targets
      .filter(
        (target) =>
          target.kind.includes('bin') &&
          (target.name.endsWith('-lambda') ||
            [
              'aura-historia-api',
              'aura-historia-mcp',
              'cognito-post-confirmation',
              'cognito-pre-sign-up',
            ].includes(target.name)),
      )
      .map((target) => target.name),
  );
  if (
    digest([...lambdas].sort()) !== digest([...expected].sort()) ||
    new Set(lambdas).size !== lambdas.length
  )
    throw new Error('Incomplete Lambda catalog');
  for (const image of images) {
    if (
      !packages.some((pkg) =>
        pkg.targets.some(
          (target) =>
            target.name === image.binary && target.kind.includes('bin'),
        ),
      ) ||
      !files.includes(image.dockerfile) ||
      !files.includes(`ci/container-images/${image.id}/smoke.sh`)
    )
      throw new Error(`Incomplete image catalog: ${image.id}`);
  }
  const dockerfiles = files.filter((file) =>
    /^src\/.*\/Dockerfile$/.test(file),
  );
  if (
    digest(dockerfiles.sort()) !==
      digest(images.map((image) => image.dockerfile).sort()) ||
    new Set(imageBinaries).size !== images.length
  )
    throw new Error('Uncatalogued deployable container');
  return {
    packages: packages
      .map((pkg) => ({
        name: pkg.name,
        targets: pkg.targets.map((target) => ({
          name: target.name,
          kind: target.kind,
          doctest: target.doctest,
          test: target.test,
        })),
      }))
      .sort((a, b) => a.name.localeCompare(b.name)),
    files: tracked.map((file) => ({
      path: file,
      sha256: bytesDigest(fs.readFileSync(path.join(root, file))),
    })),
    lambdas,
    images,
  };
}

function assertFullTests(evidence, expected = {}) {
  validateTestEvidence(evidence, expected);
  if (
    digest([...evidence.requiredJobs].sort()) !==
    digest([...REQUIRED_JOBS].sort())
  )
    throw new Error('Release evidence does not contain the trusted full suite');
  return evidence;
}
function evidence({
  sourceSha,
  workflowRevision,
  invocationRevision,
  runId,
  runAttempt,
  needs,
  fingerprints,
  completedAt = new Date().toISOString(),
}) {
  const results = Object.fromEntries(
    REQUIRED_JOBS.map((name) => [name, needs[name]?.result || 'missing']),
  );
  const result = {
    version: 1,
    repository: 'aura-historia/backend',
    sourceSha,
    workflowRevision,
    invocationRevision,
    runId,
    runAttempt: Number(runAttempt),
    ...fingerprints,
    requiredJobs: [...REQUIRED_JOBS],
    jobResults: results,
    reports: [
      `https://github.com/aura-historia/backend/actions/runs/${runId}/attempts/${runAttempt}`,
    ],
    completedAt,
  };
  return assertFullTests(result, {
    sourceSha,
    workflowRevision,
    invocationRevision,
    runId,
    runAttempt: Number(runAttempt),
  });
}
if (require.main === module) {
  const [command, root = '.'] = process.argv.slice(2);
  if (command === 'inventory') {
    const result = inventory(path.resolve(root));
    const fingerprints = {
      inventoryDigest: digest(result),
      toolchainDigest: bytesDigest(
        fs.readFileSync(path.join(root, 'rust-toolchain.toml')),
      ),
      configDigest: digest(
        result.files.filter(
          (file) =>
            file.path.startsWith('.github/') || file.path.startsWith('ci/'),
        ),
      ),
    };
    fs.writeFileSync(
      process.env.DELIVERY_FINGERPRINTS || 'fingerprints.json',
      JSON.stringify(fingerprints, null, 2) + '\n',
    );
    console.log(JSON.stringify(fingerprints));
  } else if (command === 'evidence') {
    const result = evidence({
      sourceSha: process.env.SOURCE_SHA,
      workflowRevision: process.env.WORKFLOW_REVISION,
      invocationRevision: process.env.INVOCATION_REVISION,
      runId: process.env.GITHUB_RUN_ID,
      runAttempt: process.env.GITHUB_RUN_ATTEMPT,
      needs: JSON.parse(process.env.NEEDS_JSON),
      fingerprints: JSON.parse(
        fs.readFileSync(
          process.env.DELIVERY_FINGERPRINTS || 'fingerprints.json',
        ),
      ),
    });
    fs.mkdirSync('delivery-evidence', { recursive: true });
    fs.writeFileSync(
      'delivery-evidence/tests.json',
      JSON.stringify(result, null, 2) + '\n',
    );
    if (process.env.GITHUB_OUTPUT)
      fs.appendFileSync(
        process.env.GITHUB_OUTPUT,
        `digest=${digest(result)}\n`,
      );
  } else throw new Error('Usage: gate.cjs inventory ROOT | evidence');
}
module.exports = { REQUIRED_JOBS, inventory, assertFullTests, evidence };

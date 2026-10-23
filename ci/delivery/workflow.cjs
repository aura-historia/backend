'use strict';
const fs = require('node:fs');
const { execFileSync } = require('node:child_process');
const {
  digest,
  validateBundle,
  validateRequest,
  assertRequestBinding,
  validateReport,
  assertDeployApproval,
  summarize,
} = require('./contracts.cjs');
const { assertFullTests } = require('./gate.cjs');
const { loadBuildConfig, validateObjects } = require('./build.cjs');
const storage = require('./storage.cjs');
const json = (file) => JSON.parse(fs.readFileSync(file));
function gh(endpoint) {
  return JSON.parse(
    execFileSync('gh', ['api', `repos/aura-historia/backend/${endpoint}`], {
      encoding: 'utf8',
      stdio: ['ignore', 'pipe', 'inherit'],
    }),
  );
}
function trustedInvocation(env = process.env) {
  if (
    env.GITHUB_REPOSITORY !== 'aura-historia/backend' ||
    env.GITHUB_REF !== 'refs/heads/develop' ||
    env.GITHUB_EVENT_NAME !== 'workflow_dispatch' ||
    !/^[a-f0-9]{40}$/.test(env.WORKFLOW_REVISION || '')
  )
    throw new Error(
      'Release must use the protected develop workflow and manual dispatch',
    );
  if (env.DELIVERY_PROTECTIONS_VERIFIED !== 'true')
    throw new Error(
      'Owner must verify environment reviewers, protected develop branch and disabled administrator bypass before enabling delivery',
    );
}
function assertControlRevision(env = process.env, run = execFileSync) {
  if (!/^[a-f0-9]{40}$/.test(env.INVOCATION_REVISION || ''))
    throw new Error('Missing actual invocation revision');
  try {
    run(
      'git',
      [
        'diff',
        '--quiet',
        env.WORKFLOW_REVISION,
        env.INVOCATION_REVISION,
        '--',
        '.github/workflows',
        '.github/actions',
        'ci/delivery',
        'ci/lambda-binaries.json',
        'ci/container-images.json',
        'ci/container-images',
      ],
      { stdio: ['ignore', 'ignore', 'ignore'] },
    );
  } catch {
    throw new Error(
      'Delivery control files differ from the approved pinned revision; update the reviewed control-plane configuration before releasing',
    );
  }
}
function environmentProtection(name, get = gh) {
  const environment = get(`environments/${name}`);
  const reviewers = environment.protection_rules?.find(
    (rule) => rule.type === 'required_reviewers',
  );
  if (
    !environment.id ||
    !reviewers?.reviewers?.length ||
    environment.can_admins_bypass !== false ||
    environment.deployment_branch_policy?.protected_branches !== true ||
    environment.deployment_branch_policy?.custom_branch_policies === true
  )
    throw new Error(
      `${name}: required reviewers, protected branches and disabled admin bypass are required`,
    );
  return environment;
}
function approvedEnvironment(name, get = gh) {
  const environment = environmentProtection(name, get);
  const reviews = get(`actions/runs/${process.env.GITHUB_RUN_ID}/approvals`);
  if (
    !reviews.some(
      (review) =>
        review.state === 'approved' &&
        review.user?.login &&
        review.environments?.some((item) => item.id === environment.id),
    )
  )
    throw new Error(`No recorded protected approval for ${name}`);
  return environment;
}
function output(name, value) {
  if (!process.env.GITHUB_OUTPUT) return;
  fs.appendFileSync(process.env.GITHUB_OUTPUT, `${name}=${value}\n`);
}
function save(file, value) {
  fs.mkdirSync('.delivery', { recursive: true });
  fs.writeFileSync(file, JSON.stringify(value, null, 2) + '\n');
}
function releaseSource(sha, root = '.') {
  if (!/^[a-f0-9]{40}$/.test(sha))
    throw new Error('Select a full lowercase source SHA');
  execFileSync('git', ['fetch', '--no-tags', 'origin', 'develop'], {
    cwd: root,
    stdio: 'inherit',
  });
  execFileSync('git', ['merge-base', '--is-ancestor', sha, 'origin/develop'], {
    cwd: root,
    stdio: 'inherit',
  });
  return sha;
}
function resolve() {
  trustedInvocation();
  const target = process.env.DELIVERY_TARGET;
  if (!['dev', 'prod'].includes(target)) throw new Error('Select dev or prod');
  if (target === 'dev' && process.env.DELIVERY_DEV_ENABLED !== 'true')
    throw new Error(
      'Dev delivery is disabled. Enable its delivery configuration separately; prod works independently',
    );
  const mode = process.env.DELIVERY_MODE;
  if (!['build', 'promote'].includes(mode))
    throw new Error('Select build or promote');
  environmentProtection(`aws-${target}`);
  let sourceSha,
    bundleDigest = null;
  if (mode === 'build') {
    environmentProtection('aws-release-build');
    if (process.env.DELIVERY_BUNDLE_DIGEST)
      throw new Error('Build mode cannot select an existing bundle');
    sourceSha = releaseSource(process.env.DELIVERY_SOURCE_SHA);
  } else {
    bundleDigest = process.env.DELIVERY_BUNDLE_DIGEST;
    if (
      !/^[a-f0-9]{64}$/.test(bundleDigest || '') ||
      process.env.DELIVERY_SOURCE_SHA
    )
      throw new Error('Promotion requires only an existing full bundle digest');
    const bundle = storage.readJson(
      process.env.DELIVERY_BUCKET,
      `bundles/${bundleDigest}.json`,
    );
    if (digest(bundle) !== bundleDigest)
      throw new Error('Bundle digest mismatch');
    validateObjects(bundle);
    sourceSha = releaseSource(bundle.sourceSha);
    if (!bundle.targets[target])
      throw new Error(`Bundle has no configured ${target} assembly`);
  }
  const selection = {
    version: 1,
    sourceSha,
    bundleDigest,
    target,
    mode,
    workflowRevision: process.env.WORKFLOW_REVISION,
    invocationRevision: process.env.INVOCATION_REVISION,
    runId: process.env.GITHUB_RUN_ID,
    runAttempt: Number(process.env.GITHUB_RUN_ATTEMPT),
    buildConfigDigest: digest(loadBuildConfig()),
  };
  selection.id = digest(selection);
  save('.delivery/selection.json', selection);
  output('source_sha', sourceSha);
  output('request_id', selection.id);
  output('bundle_digest', bundleDigest || '');
  if (process.env.GITHUB_STEP_SUMMARY)
    fs.appendFileSync(
      process.env.GITHUB_STEP_SUMMARY,
      `## Selected release\n\nSource: \`${sourceSha}\`\n\nTarget: **${target}**\n\nMode: ${mode}\n\nBundle: ${bundleDigest || 'new approved build'}\n\nFull tests run before build approval.\n`,
    );
}
function request(selection, tests, scope, bundleDigest) {
  const value = {
    version: 1,
    id: selection.id,
    repository: 'aura-historia/backend',
    sourceSha: selection.sourceSha,
    target: selection.target,
    workflowRevision: selection.workflowRevision,
    invocationRevision: selection.invocationRevision,
    runId: selection.runId,
    runAttempt: selection.runAttempt,
    testEvidenceDigest: digest(tests),
    buildConfigDigest: selection.buildConfigDigest,
    bundleDigest: bundleDigest || null,
    authorization: {
      scope,
      environment:
        scope === 'BUILD' ? 'aws-release-build' : `aws-${selection.target}`,
      approvedAt: new Date().toISOString(),
    },
  };
  return validateRequest(value);
}
function selected() {
  trustedInvocation();
  const selection = json('.delivery/selection.json');
  if (
    selection.workflowRevision !== process.env.WORKFLOW_REVISION ||
    selection.invocationRevision !== process.env.INVOCATION_REVISION ||
    selection.runId !== process.env.GITHUB_RUN_ID ||
    selection.runAttempt !== Number(process.env.GITHUB_RUN_ATTEMPT)
  )
    throw new Error(
      'Selection belongs to a different trusted workflow attempt; rerun all jobs',
    );
  return selection;
}
function currentTests(selection) {
  return assertFullTests(json('.delivery/tests.json'), {
    sourceSha: selection.sourceSha,
    workflowRevision: selection.workflowRevision,
    invocationRevision: selection.invocationRevision,
    runId: selection.runId,
    runAttempt: selection.runAttempt,
  });
}
function startPipeline(name, request, extra = {}, sourceSha) {
  if (!name) throw new Error('Missing configured native pipeline name');
  const bucket = process.env.DELIVERY_BUCKET;
  const key =
    request.authorization.scope === 'BUILD'
      ? `requests/build/${request.id}.json`
      : `requests/${request.target}/${request.id}.json`;
  const listed = storage.aws('s3api', 'list-objects-v2', [
    '--bucket',
    bucket,
    '--prefix',
    key,
  ]);
  if (listed.Contents?.some((object) => object.Key === key)) {
    const existing = storage.readJson(bucket, key);
    const replay = {
      ...request,
      authorization: {
        ...request.authorization,
        approvedAt: existing.authorization.approvedAt,
      },
    };
    if (digest(existing) !== digest(replay))
      throw new Error('Existing immutable request differs from retry');
    request = existing;
  }
  const reference = storage.putImmutableJson(bucket, key, request);
  const variables = {
    RequestKey: key,
    RequestVersion: reference.versionId,
    RequestDigest: digest(request),
    ...extra,
  };
  const args = [
    '--name',
    name,
    '--client-request-token',
    digest({ request: digest(request), pipeline: name }),
    '--variables',
    JSON.stringify(
      Object.entries(variables).map(([name, value]) => ({ name, value })),
    ),
  ];
  if (!process.env.DELIVERY_CONTROL_VERSION)
    throw new Error('Missing immutable control source object version');
  const revisions = [
    {
      actionName: 'ControlSource',
      revisionType: 'S3_OBJECT_VERSION_ID',
      revisionValue: process.env.DELIVERY_CONTROL_VERSION,
    },
  ];
  if (sourceSha)
    revisions.push({
      actionName: 'CandidateSource',
      revisionType: 'COMMIT_ID',
      revisionValue: sourceSha,
    });
  args.push('--source-revisions', JSON.stringify(revisions));
  const started = storage.aws('codepipeline', 'start-pipeline-execution', args);
  output('execution_id', started.pipelineExecutionId);
  if (process.env.GITHUB_STEP_SUMMARY)
    fs.appendFileSync(
      process.env.GITHUB_STEP_SUMMARY,
      `\nNative pipeline **${name}**: \`${started.pipelineExecutionId}\`. Cancelling this monitor does not cancel AWS or clear an environment hold.\n`,
    );
  return started.pipelineExecutionId;
}
function build() {
  const selection = selected(),
    tests = currentTests(selection);
  if (selection.mode !== 'build')
    throw new Error('Promotion cannot start a builder');
  approvedEnvironment('aws-release-build');
  const value = request(selection, tests, 'BUILD');
  assertRequestBinding(value, tests);
  storage.putImmutableJson(
    process.env.DELIVERY_BUCKET,
    `requests/build/${value.id}/tests.json`,
    tests,
  );
  const executionId = startPipeline(
    process.env.DELIVERY_BUILD_PIPELINE,
    value,
    {},
    value.sourceSha,
  );
  save('.delivery/build-execution.json', {
    executionId,
    buildId: digest({
      sourceSha: value.sourceSha,
      configDigest: value.buildConfigDigest,
    }),
  });
}
async function waitPipeline(name, executionId, timeoutSeconds = 7200) {
  const deadline = Date.now() + timeoutSeconds * 1000;
  while (Date.now() < deadline) {
    const execution = storage.aws('codepipeline', 'get-pipeline-execution', [
      '--pipeline-name',
      name,
      '--pipeline-execution-id',
      executionId,
    ]).pipelineExecution;
    if (execution.status === 'Succeeded') return execution;
    if (
      ['Failed', 'Stopped', 'Superseded', 'Cancelled'].includes(
        execution.status,
      )
    )
      throw new Error(
        `Native execution ${executionId} ${execution.status}; inspect AWS and retained hold before retrying`,
      );
    await new Promise((resolve) => setTimeout(resolve, 30000));
  }
  throw new Error(
    `Monitoring deadline reached; AWS execution ${executionId} continues independently. Inspect its status before rerunning`,
  );
}
async function waitBuild() {
  const execution = json('.delivery/build-execution.json');
  await waitPipeline(
    process.env.DELIVERY_BUILD_PIPELINE,
    execution.executionId,
  );
  const completion = storage.readJson(
    process.env.DELIVERY_BUCKET,
    `builds/${execution.buildId}/complete.json`,
  );
  const bundle = storage.readJson(
    process.env.DELIVERY_BUCKET,
    `bundles/${completion.bundleDigest}.json`,
  );
  if (digest(bundle) !== completion.bundleDigest)
    throw new Error('Corrupt build completion');
  validateBundle(bundle);
  output('bundle_digest', completion.bundleDigest);
}
function deploy() {
  const selection = selected(),
    tests = currentTests(selection),
    prepared = json('.delivery/approval.json'),
    report = json('.delivery/report.json');
  approvedEnvironment(`aws-${selection.target}`);
  validateReport(report);
  const value = request(selection, tests, 'DEPLOY', prepared.bundleDigest);
  const bundle = storage.readJson(
    process.env.DELIVERY_BUCKET,
    `bundles/${prepared.bundleDigest}.json`,
  );
  assertRequestBinding(value, tests, bundle);
  if (
    digest(report) !== prepared.reportDigest ||
    report.requestId !== selection.id ||
    report.target !== selection.target
  )
    throw new Error('Approved report differs from current request');
  let observations =
    JSON.parse(process.env.DELIVERY_MANUAL_COMPLETION || '{}').observations ||
    [];
  if (report.manualPrerequisites.length && !observations.length) {
    const external = storage.readJson(
      process.env.DELIVERY_BUCKET,
      `reports/manual/${selection.id}.json`,
    );
    observations = external.observations;
  }
  const manual = {
    target: selection.target,
    reportDigest: prepared.reportDigest,
    prerequisiteDigest: report.prerequisiteDigest,
    confirmed: true,
    observations,
  };
  assertDeployApproval(value, report, manual);
  storage.putImmutableJson(
    process.env.DELIVERY_BUCKET,
    `requests/${value.target}/${value.id}/tests.json`,
    tests,
  );
  storage.putImmutableJson(
    process.env.DELIVERY_BUCKET,
    `requests/${value.target}/${value.id}-manual.json`,
    manual,
  );
  const reference = storage.putImmutableJson(
    process.env.DELIVERY_BUCKET,
    `reports/${digest(report)}.json`,
    report,
  );
  startPipeline(process.env.DELIVERY_DEPLOY_PIPELINE, value, {
    BundleDigest: prepared.bundleDigest,
    ReportKey: reference.key,
    ReportVersion: reference.versionId,
    ReportDigest: prepared.reportDigest,
  });
  if (process.env.GITHUB_STEP_SUMMARY)
    fs.appendFileSync(
      process.env.GITHUB_STEP_SUMMARY,
      `\n${summarize(value, bundle, report)}\n`,
    );
}
async function main() {
  assertControlRevision();
  const command = process.argv[2];
  if (command === 'resolve') resolve();
  else if (command === 'build') build();
  else if (command === 'wait-build') await waitBuild();
  else if (command === 'deploy') deploy();
  else if (command === 'status')
    await waitPipeline(
      process.env.DELIVERY_PIPELINE,
      process.env.DELIVERY_EXECUTION_ID,
      60,
    );
  else
    throw new Error(
      'Usage: workflow.cjs resolve|build|wait-build|deploy|status',
    );
}
if (require.main === module)
  main().catch((error) => {
    console.error(error.message);
    process.exitCode = 1;
  });
module.exports = {
  trustedInvocation,
  assertControlRevision,
  approvedEnvironment,
  environmentProtection,
  request,
  releaseSource,
};

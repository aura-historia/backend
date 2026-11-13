'use strict';
const test = require('node:test'),
  assert = require('node:assert/strict'),
  fs = require('node:fs');
const {
  trustedInvocation,
  assertControlRevision,
  environmentProtection,
  request,
} = require('./workflow.cjs');
const { assertDeployApproval } = require('./contracts.cjs');
const f = require('./fixtures.cjs');
const valid = {
  GITHUB_REPOSITORY: 'aura-historia/backend',
  GITHUB_REF: 'refs/heads/develop',
  GITHUB_EVENT_NAME: 'workflow_dispatch',
  WORKFLOW_REVISION: f.sha,
  DELIVERY_PROTECTIONS_VERIFIED: 'true',
};
test('privileged entry rejects non-manual, untrusted branch and unverified environment protections', () => {
  trustedInvocation(valid);
  for (const change of [
    { GITHUB_EVENT_NAME: 'push' },
    { GITHUB_EVENT_NAME: 'pull_request' },
    { GITHUB_REF: 'refs/tags/20261023-0000' },
    { GITHUB_REPOSITORY: 'fork/backend' },
    { DELIVERY_PROTECTIONS_VERIFIED: 'false' },
  ])
    assert.throws(() => trustedInvocation({ ...valid, ...change }));
  const environment = {
    id: 1,
    protection_rules: [{ type: 'required_reviewers', reviewers: [{ id: 1 }] }],
    deployment_branch_policy: {
      protected_branches: true,
      custom_branch_policies: false,
    },
    can_admins_bypass: false,
  };
  environmentProtection('aws-prod', () => environment);
  for (const change of [
    { protection_rules: [] },
    { can_admins_bypass: true },
    { can_admins_bypass: undefined },
    { deployment_branch_policy: null },
  ])
    assert.throws(() =>
      environmentProtection('aws-prod', () => ({ ...environment, ...change })),
    );
});
test('build-cost authorization cannot authorize target mutation', () => {
  const selection = {
    id: f.hash,
    sourceSha: f.sha,
    target: 'prod',
    workflowRevision: f.sha,
    invocationRevision: f.sha,
    runId: '42',
    runAttempt: 1,
    buildConfigDigest: f.hash,
  };
  const build = request(selection, f.tests(), 'BUILD');
  assert.equal(build.authorization.environment, 'aws-release-build');
  assert.throws(() => assertDeployApproval(build, f.report()));
});
test('the only release workflow is manual and target approval follows complete tests and the separate build gate', () => {
  const workflow = fs.readFileSync('.github/workflows/deploy.yml', 'utf8');
  assert.match(workflow, /workflow_dispatch:/);
  assert.doesNotMatch(workflow, /^  (push|pull_request|schedule):/m);
  assert.match(workflow, /environment: aws-release-build/);
  assert.match(workflow, /name: aws-\$\{\{ inputs.target \}\}/);
  assert.match(workflow, /uses: \.\/\.github\/workflows\/release-tests.yml/);
  assert.match(workflow, /needs: \[resolve, tests\]/);
  assert.match(workflow, /needs: \[resolve, prepare\]/);
  const deploy = workflow.slice(workflow.indexOf("\n  deploy:\n"));
  const condition = deploy.match(/^\s+if: (.+)$/m)?.[1];
  assert.ok(condition);
  for (const required of [
    "!cancelled()", "needs.resolve.result == 'success'",
    "needs.prepare.result == 'success'", "needs.prepare.outputs.verdict == 'ROUTINE'",
  ])
    assert.ok(condition.includes(required));
  assert.doesNotMatch(
    workflow,
    /CI_DEPLOY_ROLE_ARN|cargo lambda|docker push|s3api put-object/,
  );
});

test('approved control code remains stable across app commits and rejects entry/helper changes', () => {
  let called = false;
  assertControlRevision(
    { WORKFLOW_REVISION: f.sha, INVOCATION_REVISION: 'b'.repeat(40) },
    (_program, args) => {
      called = true;
      assert.ok(args.includes('ci/delivery'));
    },
  );
  assert.equal(called, true);
  assert.throws(() =>
    assertControlRevision(
      { WORKFLOW_REVISION: f.sha, INVOCATION_REVISION: 'b'.repeat(40) },
      () => {
        throw Error('changed control');
      },
    ),
  );
});

test('the crawler release pins every action and refuses an existing release tag', () => {
  const workflow = fs.readFileSync('.github/workflows/crawler-release.yml', 'utf8');
  const uses = [...workflow.matchAll(/^\s+- uses: (\S+)/gm)].map((m) => m[1]);
  assert.ok(uses.length > 0);
  for (const action of uses.filter((u) => !u.startsWith('./')))
    assert.match(action, /@[0-9a-f]{40}$/, `${action} is not pinned to a commit`);
  // gh release create attaches to an existing tag and ignores --target: check before the build
  // and again right before publishing.
  const tagCheck = /git ls-remote --tags \S+ "refs\/tags\/crawler-\$\{SOURCE_SHA\}"/;
  const resolve = workflow.slice(workflow.indexOf('\n  resolve:\n'), workflow.indexOf('\n  tests:\n'));
  const publish = workflow.slice(workflow.indexOf('\n  publish:\n'));
  assert.match(resolve, tagCheck);
  assert.ok(publish.search(tagCheck) >= 0 && publish.search(tagCheck) < publish.indexOf('gh release create'));
});

test('CI runs the crawler deploy script tests', () => {
  for (const file of ['integrate.yml', 'crawler-release.yml']) {
    const workflow = fs.readFileSync(`.github/workflows/${file}`, 'utf8');
    assert.match(workflow, /run: bash src\/crawler\/scripts\/linux\/deploy\.test\.sh/, file);
  }
});

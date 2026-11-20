'use strict';
const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const root = path.resolve(__dirname, '..');
const read = name => fs.readFileSync(path.join(root,name),'utf8');

test('automatic image and email workflows retain local checks without an AWS publishing identity', () => {
  const images = read('.github/workflows/container-images.yml');
  const templates = read('.github/workflows/mjml-templates.yml');
  for (const workflow of [images,templates]) {
    assert.doesNotMatch(workflow,/id-token:\s*write|configure-aws-credentials|aws-push-|aws ecr|aws s3api|StartPipelineExecution/);
    assert.match(workflow,/workflow_call:/);
    assert.match(workflow,/inputs.source_sha \|\| github.sha/);
  }
  assert.match(images,/ci\/container-images\.cjs validate/);
  assert.match(images,/docker build/);
  assert.match(images,/ci\/container-images\/\$\{IMAGE_ID\}\/smoke\.sh/);
  assert.match(templates,/Compile every MJML template/);
  assert.match(templates,/generate:cognito-verification/);
});

test('the deploy workflow publishes only tested images and deploys them through the protected environment', () => {
  const deploy = read('.github/workflows/deploy.yml');
  assert.match(deploy, /^on:\n  workflow_dispatch:/m);
  assert.doesNotMatch(deploy, /^  (push|pull_request|schedule):/m);
  // AWS credentials only in the approved deploy job, and only for publishing the trigger zips.
  assert.equal(deploy.match(/id-token:\s*write/g)?.length, 1);
  assert.match(deploy, /role-to-assume: \$\{\{ secrets\.PROD_TRIGGER_PUBLISHER_ROLE_ARN \}\}/);
  assert.match(deploy, /--if-none-match '\*'/);
  assert.match(deploy, /uses: \.\/\.github\/workflows\/container-images\.yml\n    with:\n      source_sha: .*\n      export_image: true/);
  // Only the publish job, which runs none of the commit's code, may write the package.
  assert.equal(deploy.match(/packages: write/g)?.length, 1);
  assert.match(deploy, /  publish:\n    name: Publish to GHCR\n    needs: \[resolve, build\]/);
  assert.match(deploy, /environment:\n      name: prod/);
  assert.match(deploy, /concurrency:\n  group: prod-deploy\n  cancel-in-progress: false/);
  assert.match(deploy, /StrictHostKeyChecking=yes/);
  assert.match(deploy, /"deploy \$\{SHA\} \$\{IMAGE\}"/);
  const images = read('.github/workflows/container-images.yml');
  assert.doesNotMatch(images, /packages: write/);
  assert.ok(images.indexOf('Export the tested image') > images.indexOf('Smoke-test the prod Compose project'), 'the image is handed over only after its smoke tests');
});

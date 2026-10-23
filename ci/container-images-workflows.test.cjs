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

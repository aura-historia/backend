'use strict';
const test = require('node:test');
const assert = require('node:assert/strict');
const { admit } = require('./aws-admission.cjs');
const { digest } = require('./contracts.cjs');
const { REQUIRED_JOBS } = require('./gate.cjs');
const fixture = require('./fixtures.cjs');

function setup() {
  const evidence = { ...fixture.tests(), requiredJobs: [...REQUIRED_JOBS],
    jobResults: Object.fromEntries(REQUIRED_JOBS.map((name) => [name, 'success'])) };
  const request = { ...fixture.request('BUILD'), testEvidenceDigest: digest(evidence) };
  const key = `requests/build/${request.id}.json`;
  const values = { RequestKey: key, RequestVersion: 'version-1', RequestDigest: digest(request),
    ExecutionId: 'execution-1', SourceSha: request.sourceSha, ControlVersion: 'trusted-control-version' };
  const event = { 'CodePipeline.job': { id: 'job-1', accountId: '123456789012', data: {
    actionConfiguration: { configuration: { UserParameters: JSON.stringify(values) } } } } };
  const objects = new Map([[key, request], [`${key.slice(0, -5)}/tests.json`, evidence]]);
  const api = async (service, operation, input) => {
    assert.equal(service, 's3');
    if (operation === 'GetObject') {
      if (!objects.has(input.Key)) throw new Error('Missing object');
      return { Body: { transformToString: async () => JSON.stringify(objects.get(input.Key)) } };
    }
    assert.equal(input.IfNoneMatch, '*');
    if (objects.has(input.Key)) throw Object.assign(new Error('Claimed'), { name: 'PreconditionFailed' });
    objects.set(input.Key, JSON.parse(input.Body)); return {};
  };
  return { request, event, values, api, config: { scope: 'BUILD', environment: 'aws-release-build',
    repository: request.repository, controlVersion: values.ControlVersion, workflowRevision: request.workflowRevision,
    buildConfigDigest: request.buildConfigDigest } };
}

test('initial or arbitrary starts fail before release compilation, and approved execution retries are idempotent', async () => {
  const f = setup();
  await assert.rejects(admit({ 'CodePipeline.job': { id: 'bootstrap' } }, f.config, f.api), /Missing authorized/);
  const first = await admit(f.event, f.config, f.api);
  assert.deepEqual(await admit(f.event, f.config, f.api), first);
  f.values.ExecutionId = 'other-execution';
  f.event['CodePipeline.job'].data.actionConfiguration.configuration.UserParameters = JSON.stringify(f.values);
  await assert.rejects(admit(f.event, f.config, f.api), /already owned/);
});

test('changed source, protected subject, control revision, request bytes and incomplete tests cannot authorize a build', async () => {
  for (const [field, value] of [['SourceSha', 'b'.repeat(40)], ['RequestDigest', 'b'.repeat(64)],
    ['ControlVersion', 'another-control'], ['RequestKey', 'requests/prod/other.json']]) {
    const f = setup(); f.values[field] = value;
    f.event['CodePipeline.job'].data.actionConfiguration.configuration.UserParameters = JSON.stringify(f.values);
    await assert.rejects(admit(f.event, f.config, f.api));
  }
  const f = setup();
  await assert.rejects(admit(f.event, { ...f.config, environment: 'aws-prod' }, f.api), /binding/);
  await assert.rejects(admit(f.event, { ...f.config, buildConfigDigest: 'c'.repeat(64) }, f.api), /configuration/);
});

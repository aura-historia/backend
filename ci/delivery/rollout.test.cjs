'use strict';
const test = require('node:test');
const assert = require('node:assert/strict');
const { createHash } = require('node:crypto');
const { runHook } = require('./codedeploy-hook.cjs');
const { verifyHealth, fingerprint } = require('./health.cjs');

test('candidate private dependency samples and actual front door are required even without organic traffic', async () => {
  let invokes = 0, fronts = 0;
  const content = JSON.stringify({ version: '0.0', Resources: [{ api: { Type: 'AWS::Lambda::Function',
    Properties: { Name: 'api-prod', Alias: 'live', CurrentVersion: '1', TargetVersion: '2' } } }] });
  const sha256 = createHash('sha256').update(content).digest('hex');
  const api = async (_service, operation, input) => {
    if (operation === 'GetDeployment') return { deploymentInfo: { deploymentId: 'd-1', applicationName: 'api',
      revision: { revisionType: 'String', string: { sha256 } } } };
    if (operation === 'GetApplicationRevision') return { applicationName: 'api',
      revision: { revisionType: 'String', string: { content, sha256 } } };
    if (operation === 'GetAlias') return { FunctionVersion: '2' };
    invokes++;
    const event = JSON.parse(input.Payload.toString());
    return { ExecutedVersion: '2', Payload: Buffer.from(JSON.stringify({ statusCode:
      event.rawPath === '/v1/ready' ? 204 : event.rawPath === '/v1/overview' ? 200 : 401, body: '{}' })) };
  };
  const config = { functionName: 'api-prod', kind: 'api', host: 'api.example.test', phase: 'post' };
  const event = { DeploymentId: 'd-1', LifecycleEventHookExecutionId: 'hook-1' };
  await runHook(event, config, api, async (url) => { fronts++; return {
    status: url.endsWith('/ready') ? 204 : url.endsWith('/overview') ? 200 : 401, text: async () => '{}' }; });
  assert.equal(invokes, 9); assert.equal(fronts, 3);
  await assert.rejects(runHook(event, config, api, async () => ({ status: 503, text: async () => '' })), /unexpected status/);
  await assert.rejects(runHook(event, config, async (service, operation, input) => operation === 'Invoke'
    ? { FunctionError: 'Unhandled', ExecutedVersion: '2' } : api(service, operation, input)), /invocation failed/);
});

test('async failure or missing telemetry prevents a verified release while intentional disabled state is preserved', async () => {
  const settings = '{}', mappings = '{}';
  const inventory = { dmsTaskArn: 'arn:task:id', dmsInstanceId: 'instance', databaseId: 'database', streamName: 'stream',
    routerMappingId: 'router', queues: [{ name: 'work', url: 'work-url', dlqUrl: 'dead-url' }] };
  const context = { target: 'prod', startedAt: '2026-10-23T00:00:00Z', inventory,
    baseline: { dmsTask: { arn: inventory.dmsTaskArn, status: 'stopped', settingsHash: fingerprint(settings), mappingsHash: fingerprint(mappings) },
      router: { uuid: 'router', state: 'Disabled', eventSourceArn: 'stream', functionArn: 'arn:lambda:region:account:function:router:1' } } };
  const api = async (_service, operation, input) => {
    if (operation === 'describe-replication-tasks') return { ReplicationTasks: [{ ReplicationTaskArn: inventory.dmsTaskArn,
      Status: 'stopped', ReplicationTaskSettings: settings, TableMappings: mappings }] };
    if (operation === 'get-event-source-mapping') return { UUID: 'router', State: 'Disabled', EventSourceArn: 'stream', FunctionArn: context.baseline.router.functionArn };
    if (operation === 'get-queue-attributes') return { Attributes: { ApproximateNumberOfMessages: input.QueueUrl === 'dead-url' ? '1' : '0', ApproximateNumberOfMessagesNotVisible: '0' } };
    return { Datapoints: [] };
  };
  const result = await verifyHealth(context, { api, now: () => '2026-10-23T00:05:00Z' });
  assert.equal(result.status, 'FAIL');
  assert.equal(result.checks.find((check) => check.id === 'OldestReplicationSlotLag').status, 'UNVERIFIED');
  assert.equal(result.checks.find((check) => check.id === 'router-activation').status, 'PASS');
  assert.equal(result.checks.find((check) => check.id === 'dlq:work').status, 'FAIL');
});

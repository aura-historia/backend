'use strict';
const test = require('node:test');
const assert = require('node:assert/strict');
const { verifyHealth, fingerprint } = require('./health.cjs');

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

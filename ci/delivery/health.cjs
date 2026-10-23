'use strict';

const { createHash } = require('node:crypto');
const { probePublic } = require('./codedeploy-hook.cjs');

function fingerprint(value) {
  const normalized = typeof value === 'string' ? JSON.parse(value) : value;
  const canonical = (item) => Array.isArray(item) ? item.map(canonical) : item && typeof item === 'object'
    ? Object.fromEntries(Object.keys(item).sort().map((key) => [key, canonical(item[key])])) : item;
  return createHash('sha256').update(JSON.stringify(canonical(normalized))).digest('hex');
}

async function metric(api, spec, since, now) {
  const result = await api('cloudwatch', 'get-metric-statistics', {
    Namespace: spec.namespace, MetricName: spec.name,
    Dimensions: Object.entries(spec.dimensions).map(([Name, Value]) => ({ Name, Value })),
    StartTime: since, EndTime: now, Period: 60, Statistics: [spec.statistic ?? 'Maximum'],
  });
  const key = spec.statistic ?? 'Maximum';
  const points = (result.Datapoints ?? []).filter((point) =>
    Date.parse(point.Timestamp) >= Date.parse(since) && Date.parse(point.Timestamp) <= Date.parse(now)
      && Number.isFinite(point[key]));
  if (!points.length) return null;
  return key === 'Minimum' ? Math.min(...points.map((point) => point[key])) : Math.max(...points.map((point) => point[key]));
}

/** Read-only observation. An idle queue is explicit evidence; missing CloudWatch samples are not zero. */
async function verifyHealth(context, { api, fetch: fetchRequest = fetch, now = () => new Date().toISOString() }) {
  const checks = [];
  const add = (id, status, detail) => checks.push({ id, status, detail });
  const inventory = context.inventory;
  const observedAt = now();
  const since = new Date(Math.max(Date.parse(context.startedAt), Date.parse(observedAt) - 600000)).toISOString();
  if (!inventory || !context.baseline || !Number.isFinite(Date.parse(context.startedAt)))
    throw new Error('Health verification requires the approved inventory and baseline.');
  const collect = async (id, fn) => {
    try { await fn(); } catch { add(id, 'UNVERIFIED', 'Read-only evidence unavailable or invalid.'); }
  };
  for (const kind of ['api', 'mcp']) await collect(`http:${kind}`, async () => {
    const host = inventory.http?.[`${kind}Host`];
    if (!host) { add(`http:${kind}`, 'UNVERIFIED', 'Approved front-door host is missing.'); return; }
    try {
      for (let round = 0; round < 3; round++) await probePublic(kind, host, fetchRequest);
      add(`http:${kind}`, 'PASS', 'Three controlled read-only front-door samples passed.');
    } catch { add(`http:${kind}`, 'FAIL', 'Read-only front-door or authentication-boundary probe failed.'); }
  });
  await collect('dms-identity', async () => {
    const result = await api('dms', 'describe-replication-tasks', {
      Filters: [{ Name: 'replication-task-arn', Values: [inventory.dmsTaskArn] }], WithoutSettings: false,
    });
    const tasks = result.ReplicationTasks ?? [];
    const task = tasks.length === 1 ? tasks[0] : null;
    const before = context.baseline.dmsTask;
    if (!task || task.ReplicationTaskArn !== before.arn || task.Status !== before.status ||
        fingerprint(task.ReplicationTaskSettings) !== before.settingsHash || fingerprint(task.TableMappings) !== before.mappingsHash) {
      add('dms-identity', 'FAIL', 'DMS identity, capture settings or prior state changed.'); return;
    }
    if (!['running', 'stopped', 'ready'].includes(task.Status)) {
      add('dms-identity', 'FAIL', 'DMS is not in a reviewed usable state.'); return;
    }
    add('dms-identity', 'PASS', `Preserved reviewed DMS state: ${task.Status}.`);
    if (task.Status !== 'running') {
      add('dms-flow', 'PASS', 'Capture was deliberately inactive; continuity is unexercised and activation is unchanged.'); return;
    }
    const tables = await api('dms', 'describe-table-statistics', { ReplicationTaskArn: task.ReplicationTaskArn });
    if (!tables.TableStatistics?.length || tables.Marker) {
      add('dms-tables', 'UNVERIFIED', 'Complete captured-table health evidence is missing.');
    } else add('dms-tables', tables.TableStatistics.some((table) => /error|suspend|cancel/i.test(table.TableState)) ? 'FAIL' : 'PASS',
      'Checked captured-table states.');
    for (const name of ['CDCLatencySource', 'CDCLatencyTarget']) {
      const value = await metric(api, { namespace: 'AWS/DMS', name, dimensions: {
        ReplicationInstanceIdentifier: inventory.dmsInstanceId,
        ReplicationTaskIdentifier: task.ReplicationTaskArn.split(':').at(-1),
      } }, since, observedAt);
      add(name, value === null ? 'UNVERIFIED' : value <= 300 ? 'PASS' : 'FAIL',
        value === null ? 'Fresh latency telemetry missing.' : `Maximum latency: ${value} seconds.`);
    }
  });
  await collect('wal-headroom', async () => {
    for (const [name, statistic, limit, direction] of [
      ['OldestReplicationSlotLag', 'Maximum', 8 * 1024 ** 3, 'max'],
      ['FreeStorageSpace', 'Minimum', 5 * 1024 ** 3, 'min'],
    ]) {
      const value = await metric(api, { namespace: 'AWS/RDS', name, statistic,
        dimensions: { DBInstanceIdentifier: inventory.databaseId } }, since, observedAt);
      add(name, value === null ? 'UNVERIFIED' : (direction === 'max' ? value < limit : value > limit) ? 'PASS' : 'FAIL',
        value === null ? 'Fresh storage/WAL telemetry missing.' : `Observed bytes: ${value}.`);
    }
  });
  await collect('router', async () => {
    const mapping = await api('lambda', 'get-event-source-mapping', { UUID: inventory.routerMappingId });
    const before = context.baseline.router;
    const preserved = mapping.UUID === before.uuid && mapping.State === before.state &&
      mapping.EventSourceArn === before.eventSourceArn && mapping.FunctionArn.split(':').slice(0, 7).join(':') ===
      before.functionArn.split(':').slice(0, 7).join(':');
    add('router-activation', preserved ? 'PASS' : 'FAIL', 'Compared router identity and intentional activation.');
    if (!preserved || mapping.State !== 'Enabled') return;
    const incoming = await metric(api, { namespace: 'AWS/Kinesis', name: 'IncomingRecords', statistic: 'Sum',
      dimensions: { StreamName: inventory.streamName } }, since, observedAt);
    const age = await metric(api, { namespace: 'AWS/Lambda', name: 'IteratorAge',
      dimensions: { FunctionName: `cdc-router-lambda-${context.target}` } }, since, observedAt);
    const readerAge = await metric(api, { namespace: 'AWS/Kinesis', name: 'GetRecords.IteratorAgeMilliseconds',
      dimensions: { StreamName: inventory.streamName } }, since, observedAt);
    const reads = await metric(api, { namespace: 'AWS/Kinesis', name: 'GetRecords.Success', statistic: 'Sum',
      dimensions: { StreamName: inventory.streamName } }, since, observedAt);
    const idle = incoming === 0 && readerAge === 0 && reads !== null && reads > 0;
    add('router-flow', age !== null ? age <= 60000 ? 'PASS' : 'FAIL' : idle ? 'PASS' : 'UNVERIFIED',
      age !== null ? `Maximum iterator age: ${age} ms.` : idle ? 'Observed healthy reader of an empty stream.' : 'Fresh flow/idle-reader evidence missing.');
    for (const name of ['DestinationDeliveryFailures', 'OnFailureDestinationDeliveredEventCount']) {
      const value = await metric(api, { namespace: 'AWS/Lambda', name, statistic: 'Sum',
        dimensions: name === 'DestinationDeliveryFailures'
          ? { FunctionName: `cdc-router-lambda-${context.target}` }
          : { EventSourceMappingUUID: inventory.routerMappingId } }, since, observedAt);
      // Failure-only metrics need not emit zero; mapping counters and flow evidence remain separately required.
      add(name, value === null || value === 0 ? 'PASS' : 'FAIL',
        value === null ? 'No failure event metric emitted; flow evidence is checked separately.' : `Failure events: ${value}.`);
    }
  });
  if (!Array.isArray(inventory.queues) || !inventory.queues.length) add('workers', 'UNVERIFIED', 'Queue inventory is missing.');
  for (const queue of inventory.queues ?? []) await collect(`queue:${queue.name}`, async () => {
    const read = async (url) => (await api('sqs', 'get-queue-attributes', { QueueUrl: url,
      AttributeNames: ['ApproximateNumberOfMessages', 'ApproximateNumberOfMessagesNotVisible'] })).Attributes;
    const [work, dead] = await Promise.all([read(queue.url), read(queue.dlqUrl)]);
    const count = (attributes, name) => attributes && /^\d+$/.test(attributes[name]) ? Number(attributes[name]) : null;
    const visible = count(work, 'ApproximateNumberOfMessages');
    const inflight = count(work, 'ApproximateNumberOfMessagesNotVisible');
    const dlq = count(dead, 'ApproximateNumberOfMessages');
    if (visible === null || inflight === null || dlq === null) {
      add(`queue:${queue.name}`, 'UNVERIFIED', 'Explicit queue counters missing.'); return;
    }
    add(`dlq:${queue.name}`, dlq === 0 ? 'PASS' : 'FAIL', `Visible dead letters: ${dlq}.`);
    const age = await metric(api, { namespace: 'AWS/SQS', name: 'ApproximateAgeOfOldestMessage',
      dimensions: { QueueName: queue.name } }, since, observedAt);
    add(`queue:${queue.name}`, visible === 0 && inflight === 0 ? 'PASS' :
      age === null ? 'UNVERIFIED' : age <= 300 ? 'PASS' : 'FAIL',
    visible === 0 && inflight === 0 ? 'Explicitly empty; worker processing was not exercised.' : `Backlog ${visible}, in flight ${inflight}, maximum age ${age}.`);
    if (queue.workerFunctionName && visible + inflight > 0) {
      const invocations = await metric(api, { namespace: 'AWS/Lambda', name: 'Invocations', statistic: 'Sum',
        dimensions: { FunctionName: queue.workerFunctionName } }, since, observedAt);
      const errors = await metric(api, { namespace: 'AWS/Lambda', name: 'Errors', statistic: 'Sum',
        dimensions: { FunctionName: queue.workerFunctionName } }, since, observedAt);
      add(`worker:${queue.name}`, invocations === null || invocations === 0 || errors === null ? 'UNVERIFIED' : errors === 0 ? 'PASS' : 'FAIL',
        'Transport invocation evidence does not prove individual jobs succeeded; retained job/version contracts remain required.');
    }
  });
  if (context.target === 'prod') await collect('representative-worker-outcomes', async () => {
    const namespace = `AuraHistoria/ProductListingIngestion/${context.target}`;
    const completed = await metric(api, { namespace, name: 'CompletedRecords', statistic: 'Sum', dimensions: {} }, since, observedAt);
    const failed = await metric(api, { namespace, name: 'FailedRecords', statistic: 'Sum', dimensions: {} }, since, observedAt);
    const unprocessed = await metric(api, { namespace, name: 'UnprocessedRecords', statistic: 'Sum', dimensions: {} }, since, observedAt);
    const ingress = inventory.queues.find((queue) => queue.workerFunctionName === `product-listing-ingestion-lambda-${context.target}`);
    if ((failed ?? 0) > 0 || (unprocessed ?? 0) > 0) add('representative-worker-outcomes', 'FAIL', 'Ingestion emitted failed or unprocessed records.');
    else if (completed !== null && completed > 0) add('representative-worker-outcomes', 'PASS', 'Observed persisted-ingress worker completion records.');
    else {
      const idle = ingress && checks.find((check) => check.id === `queue:${ingress.name}` && check.status === 'PASS' && check.detail.startsWith('Explicitly empty'));
      add('representative-worker-outcomes', idle ? 'PASS' : 'UNVERIFIED', idle
        ? 'Ingress is explicitly idle; no worker business outcome was exercised.' : 'Required ingress completion evidence is missing while its queue is not proven idle.');
    }
  });
  return { version: 1, status: checks.some((check) => check.status === 'FAIL') ? 'FAIL' :
    checks.some((check) => check.status === 'UNVERIFIED') ? 'UNVERIFIED' : 'PASS', observedAt, checks,
  coverage: ['Read-only dependency/front-door probes run in CodeDeploy hooks.',
    'No production fixture writes or worker side effects are generated. Empty queues do not prove worker business outcomes.',
    'SQL, messages, external effects and completed stacks cannot be undone by alias rollback.'] };
}

module.exports = { verifyHealth, metric, fingerprint };

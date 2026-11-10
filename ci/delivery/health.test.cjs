'use strict';
const test = require('node:test'),
  assert = require('node:assert/strict');
const { verifyHealth } = require('./health.cjs');

const STARTED = '2026-11-10T16:00:00.000Z';
const OBSERVED = '2026-11-10T16:20:00.000Z';
const QUEUE = 'aura-worker-product-translation-prod';
const DLQ = 'aura-worker-product-translation-dlq-prod';

/** One worker queue whose counters and CloudWatch samples are given per metric name. */
function observe({ visible, inflight, dead, metrics }) {
  const api = async (service, operation, input) => {
    if (service === 'sqs' && operation === 'get-queue-attributes') {
      return { Attributes: input.QueueUrl.endsWith(`/${DLQ}`)
        ? { ApproximateNumberOfMessages: String(dead), ApproximateNumberOfMessagesNotVisible: '0' }
        : { ApproximateNumberOfMessages: String(visible), ApproximateNumberOfMessagesNotVisible: String(inflight) } };
    }
    if (service === 'cloudwatch') {
      const queue = input.Dimensions.find((dimension) => dimension.Name === 'QueueName')?.Value;
      const samples = metrics[`${queue}:${input.MetricName}`];
      if (!samples) return { Datapoints: [] };
      const statistic = input.Statistics[0];
      return { Datapoints: samples.map(([at, value]) => ({ Timestamp: at, [statistic]: value })) };
    }
    throw new Error(`unexpected ${service} ${operation}`);
  };
  return verifyHealth({
    target: 'dev',
    startedAt: STARTED,
    inventory: { queues: [{ name: QUEUE, url: `https://sqs.example/123/${QUEUE}`, dlqUrl: `https://sqs.example/123/${DLQ}` }] },
    baseline: { dmsTask: {}, router: {} },
  }, { api, now: () => OBSERVED });
}

const check = (health, id) => health.checks.find((item) => item.id === id);
const minute = (offset) => new Date(Date.parse(OBSERVED) - offset * 60000).toISOString();

test('a saturated worker that keeps acknowledging passes however old its backlog is', async () => {
  const health = await observe({ visible: 2165, inflight: 29, dead: 0, metrics: {
    [`${QUEUE}:NumberOfMessagesReceived`]: [[minute(5), 430], [minute(4), 425]],
    [`${QUEUE}:NumberOfMessagesDeleted`]: [[minute(5), 428], [minute(4), 420]],
    [`${QUEUE}:ApproximateAgeOfOldestMessage`]: [[minute(5), 1340]],
  } });
  assert.equal(check(health, `queue:${QUEUE}`).status, 'PASS');
  assert.match(check(health, `queue:${QUEUE}`).detail, /deleted 848 of 855 received/);
  assert.equal(check(health, `dlq:${QUEUE}`).status, 'PASS');
});

test('a throttled worker above half acknowledgement passes and a failing worker fails', async () => {
  const run = (deleted) => observe({ visible: 185, inflight: 121, dead: 0, metrics: {
    [`${QUEUE}:NumberOfMessagesReceived`]: [[minute(3), 1000]],
    [`${QUEUE}:NumberOfMessagesDeleted`]: [[minute(3), deleted]],
  } });
  assert.equal(check(await run(780), `queue:${QUEUE}`).status, 'PASS');
  assert.equal(check(await run(120), `queue:${QUEUE}`).status, 'FAIL');
  assert.equal(check(await run(0), `queue:${QUEUE}`).status, 'FAIL');
});

test('missing progress samples stay unverified and an empty queue is explicit evidence', async () => {
  const unknown = await observe({ visible: 10, inflight: 0, dead: 0, metrics: {} });
  assert.equal(check(unknown, `queue:${QUEUE}`).status, 'UNVERIFIED');
  const empty = await observe({ visible: 0, inflight: 0, dead: 0, metrics: {} });
  assert.equal(check(empty, `queue:${QUEUE}`).status, 'PASS');
});

test('only dead letters added since the operation started fail, and an unsampled start fails closed', async () => {
  const run = (dead, before) => observe({ visible: 0, inflight: 0, dead, metrics: before === undefined ? {} : {
    [`${DLQ}:ApproximateNumberOfMessagesVisible`]: [[new Date(Date.parse(STARTED) - 120000).toISOString(), before]],
  } });
  assert.equal(check(await run(3, 3), `dlq:${QUEUE}`).status, 'PASS');
  assert.equal(check(await run(5, 3), `dlq:${QUEUE}`).status, 'FAIL');
  assert.equal(check(await run(2), `dlq:${QUEUE}`).status, 'FAIL');
  assert.equal(check(await run(0), `dlq:${QUEUE}`).status, 'PASS');
});

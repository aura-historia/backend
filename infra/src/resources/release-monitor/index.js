'use strict';

function observation(state, now) {
  if (!state) return { overdue: 0, seconds: 0 };
  if (state.version !== 1) throw new Error('Unsupported release state version');
  const release = state.active;
  if (!release?.pauseOwned || !release.pauseAt || release.resumedAt) return { overdue: 0, seconds: 0 };
  const seconds = Math.max(0, (now - Date.parse(release.pauseAt)) / 1000);
  if (!Number.isFinite(seconds) || !Number.isFinite(release.pauseBudgetSeconds)) throw new Error('Invalid pause evidence');
  return { overdue: Number(seconds > release.pauseBudgetSeconds), seconds };
}
let clients;
exports.handler = async () => {
  const { S3Client, GetObjectCommand } = require('@aws-sdk/client-s3');
  const { CloudWatchClient, PutMetricDataCommand } = require('@aws-sdk/client-cloudwatch');
  clients ??= { s3: new S3Client({ region: process.env.AWS_REGION, maxAttempts: 2 }), cloudwatch: new CloudWatchClient({ region: process.env.AWS_REGION, maxAttempts: 2 }) };
  for (const stage of ['dev', 'prod']) {
    let values, failed = 0;
    try {
      let state;
      try {
        const result = await clients.s3.send(new GetObjectCommand({ Bucket: process.env.STATE_BUCKET, ExpectedBucketOwner: process.env.ACCOUNT, Key: `stages/${process.env.ACCOUNT}/${process.env.AWS_REGION}/${stage}.json` }), { abortSignal: AbortSignal.timeout(10000) });
        state = JSON.parse(await result.Body.transformToString());
      } catch (error) { if (error.name !== 'NoSuchKey') throw error; }
      values = observation(state, Date.now());
    } catch { failed = 1; values = { overdue: 1, seconds: 0 }; }
    await clients.cloudwatch.send(new PutMetricDataCommand({ Namespace: 'AuraHistoria/Release', MetricData: [
      { MetricName: 'PauseOverdue', Value: values.overdue }, { MetricName: 'PauseSeconds', Value: values.seconds }, { MetricName: 'MonitorError', Value: failed },
    ].map(metric => ({ ...metric, Dimensions: [{ Name: 'Stage', Value: stage }] })) }), { abortSignal: AbortSignal.timeout(10000) });
  }
};
exports.observation = observation;

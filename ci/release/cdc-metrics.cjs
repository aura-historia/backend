'use strict';
function metric(ctx, namespace, name, dimensions, since, statistic, api) {
  const result = api(ctx, 'cloudwatch', 'get-metric-statistics', [
    '--namespace',
    namespace,
    '--metric-name',
    name,
    '--dimensions',
    ...Object.entries(dimensions).map(([key, value]) => `Name=${key},Value=${value}`),
    '--start-time',
    since,
    '--end-time',
    new Date().toISOString(),
    '--period',
    '60',
    '--statistics',
    statistic,
  ]);
  const points = result.Datapoints?.filter(
    (p) => Date.parse(p.Timestamp) >= Date.parse(since) && Number.isFinite(p[statistic]),
  );
  return points?.length ? Math.max(...points.map((p) => p[statistic])) : null;
}
function observe(ctx, dimensions, enabled, since, streamName, api) {
  const window = new Date(Math.max(Date.parse(since), Date.now() - 300000)).toISOString();
  const source = metric(ctx, 'AWS/DMS', 'CDCLatencySource', dimensions, window, 'Maximum', api);
  const target = metric(ctx, 'AWS/DMS', 'CDCLatencyTarget', dimensions, window, 'Maximum', api);
  const rows = metric(ctx, 'AWS/DMS', 'CDCThroughputRowsSource', dimensions, window, 'Maximum', api);
  let router = null,
    idleStream = null;
  if (enabled === 'true') {
    router = metric(
      ctx,
      'AWS/Lambda',
      'IteratorAge',
      { FunctionName: `cdc-router-lambda-${ctx.stage}` },
      window,
      'Maximum',
      api,
    );
    idleStream = metric(
      ctx,
      'AWS/Kinesis',
      'IncomingRecords',
      { StreamName: streamName },
      window,
      'Sum',
      api,
    );
  }
  return { source, target, rows, router, idleStream };
}
function ready({ source, target, rows, router, idleStream }, enabled, progress) {
  return (
    source !== null &&
    target !== null &&
    source <= 300 &&
    target <= 300 &&
    (progress || rows === 0) &&
    (enabled === 'false' || (router !== null && router <= 60000) || (router === null && idleStream === 0))
  );
}
module.exports = { metric, observe, ready };

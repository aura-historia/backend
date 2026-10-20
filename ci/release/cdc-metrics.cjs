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
function routerMapping(ctx, stacks, streamName, api) {
  const compute = stacks.compute;
  const exported = compute?.stack.Outputs?.filter(
    (output) => output.ExportName === `aura-historia-cdc-router-event-source-mapping-id-${ctx.stage}`,
  );
  const uuid = exported?.length === 1 ? exported[0].OutputValue : null;
  if (
    !uuid ||
    !compute.resources.some(
      (resource) =>
        resource.ResourceType === 'AWS::Lambda::EventSourceMapping' && resource.PhysicalResourceId === uuid,
    )
  )
    throw new Error('Cannot resolve the stage-owned CDC router mapping for health verification.');
  const mapping = api(ctx, 'lambda', 'get-event-source-mapping', ['--uuid', uuid]);
  const functionArn = `arn:aws:lambda:${ctx.region}:${ctx.account}:function:cdc-router-lambda-${ctx.stage}`;
  if (
    mapping.UUID !== uuid ||
    mapping.EventSourceArn !== `arn:aws:kinesis:${ctx.region}:${ctx.account}:stream/${streamName}` ||
    !(mapping.FunctionArn === functionArn || mapping.FunctionArn?.startsWith(`${functionArn}:`))
  )
    throw new Error('CDC router mapping does not match the stage function and stream.');
  if (mapping.State !== 'Enabled')
    throw new Error(
      `CDC router mapping is ${mapping.State}; restore configured activation before verifying capture.`,
    );
  return uuid;
}
function observe(ctx, dimensions, enabled, since, streamName, api) {
  const window = new Date(Math.max(Date.parse(since), Date.now() - 300000)).toISOString();
  const source = metric(ctx, 'AWS/DMS', 'CDCLatencySource', dimensions, window, 'Maximum', api);
  const target = metric(ctx, 'AWS/DMS', 'CDCLatencyTarget', dimensions, window, 'Maximum', api);
  const rows = metric(ctx, 'AWS/DMS', 'CDCThroughputRowsSource', dimensions, window, 'Maximum', api);
  let router = null,
    idleStream = null,
    readerAge = null,
    readRecords = null,
    successfulReads = null;
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
    if (router === null && idleStream === null && rows === 0) {
      const stream = { StreamName: streamName };
      readerAge = metric(
        ctx,
        'AWS/Kinesis',
        'GetRecords.IteratorAgeMilliseconds',
        stream,
        window,
        'Maximum',
        api,
      );
      readRecords = metric(ctx, 'AWS/Kinesis', 'GetRecords.Records', stream, window, 'Maximum', api);
      successfulReads = metric(ctx, 'AWS/Kinesis', 'GetRecords.Success', stream, window, 'Sum', api);
    }
  }
  return { source, target, rows, router, idleStream, readerAge, readRecords, successfulReads };
}
function idle(metrics) {
  return (
    metrics.idleStream === 0 ||
    (metrics.rows === 0 &&
      metrics.readerAge === 0 &&
      metrics.readRecords === 0 &&
      metrics.successfulReads > 0)
  );
}
function pending(metrics, enabled, progress) {
  const { source, target, rows, router } = metrics;
  const waiting = [];
  if (source === null) waiting.push('fresh DMS source latency');
  else if (source > 300) waiting.push('DMS source latency <=300s');
  if (target === null) waiting.push('fresh DMS target latency');
  else if (target > 300) waiting.push('DMS target latency <=300s');
  if (!progress && rows !== 0) waiting.push('checkpoint movement or fresh zero source throughput');
  if (enabled === 'true' && !(router !== null ? router <= 60000 : idle(metrics)))
    waiting.push(
      router === null ? 'fresh router age or verified idle stream reads' : 'router iterator age <=60s',
    );
  return waiting;
}
function describe(metrics) {
  return Object.entries(metrics)
    .map(([name, value]) => `${name}=${value === null ? 'missing' : value}`)
    .join(', ');
}
module.exports = { metric, routerMapping, observe, idle, pending, describe };

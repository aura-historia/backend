'use strict';
const { aws, sleep, hash } = require('./io.cjs');
const cdcMetrics = require('./cdc-metrics.cjs');
function read(ctx, arn, api = aws) {
  const response = api(ctx, 'dms', 'describe-replication-tasks', [
    '--filters',
    `Name=replication-task-arn,Values=${arn}`,
    '--without-settings',
  ]);
  const task = response.ReplicationTasks?.length === 1 ? response.ReplicationTasks[0] : null;
  if (!task || task.ReplicationTaskArn !== arn || task.MigrationType !== 'cdc')
    throw new Error(
      'Owned DMS task is missing, replaced or not CDC-only. Use lost-slot/task recovery; do not start a new task.',
    );
  if (
    !Number.isFinite(Date.parse(task.ReplicationTaskStats?.StartDate)) ||
    !/^checkpoint:V1#\S+$/.test(task.RecoveryCheckpoint || '')
  )
    throw new Error(
      'DMS has never run or has no supported recovery checkpoint. Use first-time activation or checkpoint recovery.',
    );
  if (task.Status === 'failed' || task.ReplicationTaskStats.TablesErrored !== 0)
    throw new Error(
      'DMS is failed or reports table errors. Inspect restricted DMS diagnostics and repair before release.',
    );
  return task;
}
function evidence(task) {
  return {
    arn: task.ReplicationTaskArn,
    source: task.SourceEndpointArn,
    target: task.TargetEndpointArn,
    instance: task.ReplicationInstanceArn,
    checkpoint: task.RecoveryCheckpoint,
    status: task.Status,
    startedAt: task.ReplicationTaskStats.StartDate,
  };
}
function topology(stacks, task) {
  for (const [field, type] of [
    ['SourceEndpointArn', 'AWS::DMS::Endpoint'],
    ['TargetEndpointArn', 'AWS::DMS::Endpoint'],
    ['ReplicationInstanceArn', 'AWS::DMS::ReplicationInstance'],
  ]) {
    if (!stacks.data.resources.some((r) => r.ResourceType === type && r.PhysicalResourceId === task[field]))
      throw new Error(`DMS ${field} is outside this stage's CloudFormation ownership.`);
  }
}
function captureConfig(ctx, task, api = aws) {
  const endpoints = [task.SourceEndpointArn, task.TargetEndpointArn].map((arn) => {
    const matches = api(ctx, 'dms', 'describe-endpoints', [
      '--filters',
      `Name=endpoint-arn,Values=${arn}`,
    ]).Endpoints;
    if (matches?.length !== 1 || matches[0].EndpointArn !== arn || matches[0].Status !== 'active')
      throw new Error('Stage capture endpoint is missing or inactive.');
    const endpoint = matches[0];
    return {
      arn,
      engine: endpoint.EngineName,
      database: endpoint.DatabaseName,
      slot: endpoint.PostgreSQLSettings?.SlotName,
      plugin: endpoint.PostgreSQLSettings?.PluginName,
      heartbeat: endpoint.PostgreSQLSettings?.HeartbeatEnable,
      stream: endpoint.KinesisSettings?.StreamArn,
    };
  });
  return hash(endpoints);
}
function sameTask(task, recorded) {
  const current = evidence(task);
  for (const key of ['arn', 'source', 'target', 'instance'])
    if (current[key] !== recorded[key])
      throw new Error(`DMS ${key} changed; capture recovery needs operator review.`);
}
function admission(task, active) {
  if (active?.dms) {
    sameTask(task, active.dms);
    if (
      !active.pauseOwned &&
      task.Status !== 'stopped' &&
      !(active.phase.startsWith('DEPLOY_') && task.Status === 'modifying')
    ) {
      throw new Error(
        'Originally stopped DMS changed lifecycle without release ownership; inspect the external operation.',
      );
    }
    const allowed = active.resumedAt
      ? ['running']
      : active.phase === 'PAUSING'
        ? ['running', 'stopping', 'stopped']
        : ['RESUMING', 'VERIFYING'].includes(active.phase)
          ? ['stopped', 'starting', 'running']
          : active.phase.startsWith('DEPLOY_')
            ? ['stopped', 'modifying']
            : ['stopped'];
    if (!allowed.includes(task.Status))
      throw new Error(
        `Unexplained DMS ${task.Status} in ${active.phase}; inspect the owned operation before continuation.`,
      );
    return;
  }
  if (!['running', 'stopped'].includes(task.Status))
    throw new Error(
      `DMS ${task.Status} has no recorded operation. Wait for or recover the owning operation before release.`,
    );
  if (task.Status === 'stopped' && !/^Stop Reason NORMAL$/i.test(task.StopReason || ''))
    throw new Error(
      'Stopped DMS task lacks an intentional normal-stop reason; inspect DMS before adoption/release.',
    );
}
async function wait(ctx, recorded, target, allowed, { api = aws, delay = sleep, attempts = 90 } = {}) {
  const deadline = Date.now() + 15 * 60 * 1000;
  for (let i = 0; i < attempts && Date.now() < deadline; i++) {
    const task = read(ctx, recorded.arn, api);
    sameTask(task, recorded);
    if (task.Status === target) return task;
    if (!allowed.includes(task.Status))
      throw new Error(`DMS ${target} wait observed unexpected ${task.Status}; ownership retained.`);
    await delay(10000);
  }
  throw new Error(
    `DMS did not reach ${target} within the 15-minute deadline. Continue the same release after inspecting DMS.`,
  );
}
async function pause(ctx, recorded, options = {}) {
  const api = options.api || aws;
  let task = read(ctx, recorded.arn, api);
  sameTask(task, recorded);
  if (task.Status === 'running') {
    try {
      api(ctx, 'dms', 'stop-replication-task', ['--replication-task-arn', recorded.arn]);
    } catch {
      /* The request may have been accepted. Reconcile, never infer success. */
    }
  } else if (!['stopped', 'stopping'].includes(task.Status))
    throw new Error(`Cannot pause DMS from ${task.Status}.`);
  return evidence(await wait(ctx, recorded, 'stopped', ['running', 'stopping'], options));
}
async function resume(ctx, recorded, options = {}) {
  const api = options.api || aws;
  const task = read(ctx, recorded.arn, api);
  sameTask(task, recorded);
  if (task.Status === 'stopped') {
    try {
      api(ctx, 'dms', 'start-replication-task', [
        '--replication-task-arn',
        recorded.arn,
        '--start-replication-task-type',
        'resume-processing',
      ]);
    } catch {
      /* Re-read after an ambiguous response, without submitting another start. */
    }
  } else if (!['running', 'starting'].includes(task.Status))
    throw new Error(`Cannot resume DMS from ${task.Status}.`);
  return evidence(await wait(ctx, recorded, 'running', ['stopped', 'starting'], options));
}
async function health(
  ctx,
  recorded,
  enabled,
  since,
  stacks,
  { api = aws, delay = sleep, attempts = 90 } = {},
) {
  const instance = api(ctx, 'dms', 'describe-replication-instances', [
    '--filters',
    `Name=replication-instance-arn,Values=${recorded.instance}`,
  ]).ReplicationInstances;
  if (instance?.length !== 1 || instance[0].ReplicationInstanceArn !== recorded.instance)
    throw new Error('Cannot resolve owned DMS instance metrics.');
  const dimensions = {
    ReplicationInstanceIdentifier: instance[0].ReplicationInstanceIdentifier,
    ReplicationTaskIdentifier: recorded.arn.split(':').at(-1),
  };
  const stream = stacks.data.resources.filter((r) => r.ResourceType === 'AWS::Kinesis::Stream');
  if (stream.length !== 1) throw new Error('Cannot resolve the stage CDC stream for recovery verification.');
  const deadline = Date.now() + 15 * 60 * 1000;
  for (let i = 0; i < attempts && Date.now() < deadline; i++) {
    const task = read(ctx, recorded.arn, api);
    sameTask(task, recorded);
    if (task.Status !== 'running')
      throw new Error(`CDC verification observed DMS ${task.Status}; release is deployed but unverified.`);
    const tables = api(ctx, 'dms', 'describe-table-statistics', [
      '--replication-task-arn',
      recorded.arn,
    ]).TableStatistics;
    if (
      !Array.isArray(tables) ||
      tables.some(
        (t) => typeof t.TableState !== 'string' || /error|suspend|cancel|does not exist/i.test(t.TableState),
      )
    )
      throw new Error(
        'CDC table statistics are missing or report errors; release is deployed but unverified.',
      );
    // Ignore old and delayed points. Idle capture is healthy only with fresh latency
    // and throughput evidence; an absent metric is never interpreted as zero.
    const metrics = cdcMetrics.observe(ctx, dimensions, enabled, since, stream[0].PhysicalResourceId, api);
    const { source, target, rows, router, idleStream } = metrics;
    const progress = task.RecoveryCheckpoint !== recorded.checkpoint;
    if (cdcMetrics.ready(metrics, enabled, progress))
      return {
        verifiedAt: new Date().toISOString(),
        checkpointHash: hash(task.RecoveryCheckpoint),
        progress,
        idleSource: rows === 0,
        sourceLatencySeconds: source,
        targetLatencySeconds: target,
        routerAgeMs: router,
        idleStream: idleStream === 0,
      };
    await delay(10000);
  }
  throw new Error(
    'CDC recovery exceeded 15 minutes: require fresh source/target latency <=300s, checkpoint progress or idle throughput, and router age <=60s or a proven idle stream. Capture may be running; ownership remains deployed-but-unverified.',
  );
}
module.exports = {
  read,
  evidence,
  topology,
  captureConfig,
  sameTask,
  admission,
  pause,
  resume,
  wait,
  health,
  metric: cdcMetrics.metric,
};

'use strict';
const { hash, aws } = require('./io.cjs');
const stacks = require('./stacks.cjs');
const dms = require('./dms.cjs');
const { protection } = require('./github.cjs');
const { requirements } = require('./requirements.cjs');
const { loadCatalog, validateDigestMap } = require('../container-images.cjs');
const { assertAvailable } = require('./state.cjs');
function monitor(ctx, active, api = aws) {
  let resumed = false;
  if (active?.phase === 'RESUMING' && active.completed.includes('DEPLOYED')) {
    const task = dms.read(ctx, active.dms.arn, api);
    dms.sameTask(task, active.dms);
    resumed = task.Status === 'running';
  }
  const response = api(ctx, 'cloudwatch', 'describe-alarms', [
    '--alarm-names',
    `aura-historia-release-${ctx.stage}-overdue`,
    `aura-historia-release-${ctx.stage}-monitor`,
  ]);
  if (
    response.MetricAlarms?.length !== 2 ||
    response.MetricAlarms.some(
      (a) =>
        a.ActionsEnabled !== true ||
        !a.AlarmActions?.length ||
        (a.StateValue !== 'OK' &&
          !(
            a.AlarmName.endsWith('-overdue') &&
            (resumed ||
              (active?.pauseAt &&
                ctx.budget > active.pauseBudgetSeconds &&
                Date.now() - Date.parse(active.pauseAt) < ctx.budget * 1000))
          )),
    )
  )
    throw new Error(
      'Release pause monitoring is absent or alarming. Confirm SNS delivery, inspect WAL/storage and repair monitoring before release; see docs/infra.md#release-recovery.',
    );
}
function validateBaseline(ctx, stored, live, targets) {
  const active = stored.state.active;
  if (
    !active &&
    (stacks.parameters(live.compute.stack).CommitSHA !== stored.state.completed.sha ||
      stacks.parameters(live.initialize.stack).CommitSHA !== stored.state.completed.sha ||
      hash(stacks.snapshot(live)) !== hash(stored.state.completed.stacks))
  )
    throw new Error(
      'Deployed stage differs from its completed baseline. Reconcile exceptional changes and adopt verified state before release.',
    );
  for (const name of stacks.names(ctx))
    if (
      active?.completed.includes(`DEPLOY_${name.toUpperCase()}`) &&
      !stacks.deployed(ctx, name, live, hash(stacks.normalized(targets[name])), active.approvedPlan.digests)
    )
      throw new Error(`Previously confirmed ${name} changed; reconcile the stage before continuation.`);
}

function validateActivation(ctx, stored, live, api) {
  const runtime = stacks.runtimeActivation(ctx, live, api);
  const expected = stored.state.active?.runtimeActivation || stored.state.completed.runtimeActivation;
  for (const [key, value] of Object.entries(expected || {}))
    if (runtime[key] !== value)
      throw new Error(`Workload activation changed at ${key}; recover before continuation.`);
  return runtime;
}

function plan(
  ctx,
  stored,
  live,
  targets,
  digests,
  { api = aws, readRequirements = requirements, protect = protection } = {},
) {
  assertAvailable(stored.state, ctx, 'release');
  validateDigestMap(digests, loadCatalog());
  if (!stored.state.completed?.sha || stored.state.adoptionRequired)
    throw new Error(
      'No trustworthy completed release baseline. Use the protected Adopt release baseline operation after verifying the deployed stage.',
    );
  const active = stored.state.active;
  const alreadyComplete = stored.state.completed.sha === ctx.sha && !active;
  const task = dms.read(ctx, stacks.taskIdentity(ctx, live), api);
  dms.topology(live, task);
  dms.admission(task, active);
  const captureConfig = dms.captureConfig(ctx, task, api);
  if (captureConfig !== stored.state.completed.captureConfig)
    throw new Error(
      'Capture endpoint/slot configuration differs from the completed baseline. Use dedicated recovery.',
    );
  if (active?.dms && active.dms.arn !== task.ReplicationTaskArn)
    throw new Error('Recorded task was replaced; dedicated recovery is required.');
  validateBaseline(ctx, stored, live, targets);
  const runtimeActivation = validateActivation(ctx, stored, live, api);
  const requirementsPlan = readRequirements(stored.state.completed.sha, ctx.sha);
  const changes = stacks.guardChanges(live, targets);
  return {
    version: 1,
    alreadyComplete,
    protection: hash(protect(ctx).protection_rules),
    account: ctx.account,
    region: ctx.region,
    stage: ctx.stage,
    sha: ctx.sha,
    ref: ctx.ref,
    baseline: stored.state.completed.sha,
    stateEtag: stored.etag,
    revision: stored.state.revision,
    run: ctx.run,
    attempt: ctx.attempt,
    pauseBudgetSeconds: ctx.budget,
    stacks: stacks.snapshot(live),
    targets: stacks.templateHashes(targets),
    digests,
    captureConfig,
    runtimeActivation,
    activation: active?.activation || stacks.activation(live),
    task: {
      arn: task.ReplicationTaskArn,
      status: task.Status,
      topology: hash([task.SourceEndpointArn, task.TargetEndpointArn, task.ReplicationInstanceArn]),
    },
    requirements: requirementsPlan,
    changes,
    continuation: active
      ? { id: active.id, phase: active.phase, completed: active.completed, pauseAt: active.pauseAt || null }
      : null,
  };
}
function render(plan) {
  const manual = plan.requirements.manual;
  return [
    '## Coordinated release plan',
    `- Account/region/stage: ${plan.account} / ${plan.region} / ${plan.stage}`,
    `- Source: ${plan.ref}; SHA: ${plan.sha}; completed baseline: ${plan.baseline}`,
    `- Plan: ${hash(plan)}; prerequisite: ${plan.requirements.id}`,
    `- DMS: ${plan.task.arn}; observed ${plan.task.status}; pause budget ${plan.pauseBudgetSeconds}s`,
    `- Continuation: ${plan.continuation ? `${plan.continuation.id}, ${plan.continuation.phase}, completed ${plan.continuation.completed.join(', ')}, pause since ${plan.continuation.pauseAt}` : 'new forward release'}`,
    '- Phases: own stage → pause DMS → network/data/initialize → migrate → compute/API/observability → verify → resume owned pause → verify CDC → complete.',
    '- Existing HTTP requests, workers, schedules and retained work continue. Approval confirms compatibility with them.',
    ...Object.entries(plan.changes).map(
      ([name, paths]) =>
        `- ${name}: ${paths.length ? paths.join(', ') : 'no template changes'} (parameter updates remain pinned to this release)`,
    ),
    ...plan.requirements.entries.map(
      (e) => `- Requirement ${e.id}: ${e.classification}; ${e.reason}; ${e.runbook}`,
    ),
    ...(manual.length
      ? [
          '### Approval also confirms completed OpenSearch preparation',
          `Approval attests these actions and validations were completed for **${plan.stage} / ${plan.sha} / ${plan.requirements.id}**, while the existing application remains compatible. These are operator attestations, not automated private-host probes.`,
          ...manual.map(
            (e) => `- **${e.id}**: ${e.action}\n  Validation: ${e.validation}\n  Runbook: ${e.runbook}`,
          ),
        ]
      : ['- No manual OpenSearch prerequisite. One ordinary release approval is required.']),
  ].join('\n');
}
module.exports = { plan, render, monitor };
